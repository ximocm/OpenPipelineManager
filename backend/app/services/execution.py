from __future__ import annotations

import json
import queue
import shlex
import subprocess
import threading
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from app.models.pipeline import PipelineStep, ValidationIssue
from app.models.state import ExecutionStatus, StepRuntimeState
from app.services.storage import ProjectStore
from app.services.validation import PLACEHOLDER_RE, effective_step_values, has_value


class ExecutionBlockedError(RuntimeError):
    def __init__(self, blockers: list[ValidationIssue]) -> None:
        self.blockers = blockers
        summary = "; ".join(format_validation_issue(issue) for issue in blockers)
        plural = "" if len(blockers) == 1 else "s"
        message = f"Execution blocked by {len(blockers)} validation blocker{plural}"
        if summary:
            message = f"{message}: {summary}"
        super().__init__(message)


class ExecutionManager:
    def __init__(self, store: ProjectStore) -> None:
        self.store = store
        self.events: queue.Queue[dict[str, Any]] = queue.Queue()
        self.thread: threading.Thread | None = None
        self.process: subprocess.Popen[str] | None = None
        self.current_step_id: str | None = None
        self.stop_requested = False
        self.lock = threading.Lock()

    def build_command(self, step: PipelineStep, params: dict[str, Any] | None = None) -> str:
        values = effective_step_values(step, params)
        placeholders = sorted(set(PLACEHOLDER_RE.findall(step.command)))
        missing = [key for key in placeholders if not has_value(values.get(key))]
        if missing:
            raise ValueError(f"Missing placeholder values: {', '.join(missing)}")

        command = _substitute_placeholders(step.command, values)
        return self._apply_environment(step.environment, command)

    def _apply_environment(self, environment: str, command: str) -> str:
        environment = environment.strip()
        if not environment:
            return command

        kind, separator, value = environment.partition(":")
        env_value = value.strip() if separator else environment
        if not env_value:
            return command

        if separator and kind in {"conda", "mamba", "micromamba"}:
            return f"{kind} run -n {shlex.quote(env_value)} {command}"
        if separator and kind == "module":
            return f"module load {shlex.quote(env_value)} && {command}"
        if separator and kind == "shell":
            return f"{env_value} && {command}"

        return f"conda run -n {shlex.quote(environment)} {command}"

    def status(self) -> ExecutionStatus:
        return ExecutionStatus(
            running=self.thread is not None and self.thread.is_alive(),
            current_step_id=self.current_step_id,
            stop_requested=self.stop_requested,
        )

    def run_step(self, step_id: str) -> ExecutionStatus:
        return self.run_steps([step_id])

    def run_selected(self, step_ids: list[str] | None = None) -> ExecutionStatus:
        if step_ids is None:
            step_ids = [step_id for step_id, state in self.store.state.items() if state.selected]
        if not step_ids:
            pipeline = self.store.get_pipeline()
            step_ids = [step.id for step in pipeline.steps if self.store.state.get(step.id, StepRuntimeState()).status != "ok"]
        return self.run_steps(step_ids)

    def run_steps(self, step_ids: list[str]) -> ExecutionStatus:
        with self.lock:
            if self.status().running:
                raise RuntimeError("Execution is already running")
            self._raise_for_validation_blockers(step_ids)
            ordered = self._topological_subset(step_ids)
            self.stop_requested = False
            self.thread = threading.Thread(target=self._run_ordered_steps, args=(ordered,), daemon=True)
            self.thread.start()
        return self.status()

    def stop(self) -> ExecutionStatus:
        self.stop_requested = True
        if self.process and self.process.poll() is None:
            self.process.terminate()
        return self.status()

    def next_event(self, timeout: float = 0.25) -> dict[str, Any] | None:
        try:
            return self.events.get(timeout=timeout)
        except queue.Empty:
            return None

    def _topological_subset(self, step_ids: list[str]) -> list[str]:
        pipeline = self.store.get_pipeline()
        step_map = pipeline.step_by_id()
        ordered: list[str] = []
        temporary: set[str] = set()
        permanent: set[str] = set()

        def visit(step_id: str) -> None:
            if step_id in permanent:
                return
            if step_id in temporary:
                raise ValueError(f"Cycle detected while scheduling: {step_id}")
            step = step_map.get(step_id)
            if step is None:
                raise KeyError(f"Unknown step: {step_id}")
            temporary.add(step_id)
            for dependency in step.dependencies:
                visit(dependency)
            temporary.remove(step_id)
            permanent.add(step_id)
            ordered.append(step_id)

        for step_id in step_ids:
            visit(step_id)
        return ordered

    def _raise_for_validation_blockers(self, step_ids: list[str]) -> None:
        issues = self.store.validate()
        relevant_step_ids = self._dependency_closure(step_ids)
        blockers = [
            issue
            for issue in issues
            if issue.severity == "blocker" and (issue.step_id is None or issue.step_id in relevant_step_ids)
        ]
        if blockers:
            raise ExecutionBlockedError(blockers)

    def _dependency_closure(self, step_ids: list[str]) -> set[str]:
        pipeline = self.store.get_pipeline()
        step_map = pipeline.step_by_id()
        seen: set[str] = set()

        def visit(step_id: str) -> None:
            if step_id in seen:
                return
            seen.add(step_id)
            step = step_map.get(step_id)
            if step is None:
                return
            for dependency in step.dependencies:
                visit(dependency)

        for step_id in step_ids:
            visit(step_id)
        return seen

    def _run_ordered_steps(self, step_ids: list[str]) -> None:
        try:
            for step_id in step_ids:
                if self.stop_requested:
                    break
                step = self.store.get_step(step_id)
                incomplete = [dependency for dependency in step.dependencies if not self.store.is_step_ok(self.store.get_step(dependency))]
                if incomplete:
                    state = self.store.state.get(step_id, StepRuntimeState())
                    state.status = "error"
                    state.message = f"Dependency is not complete: {', '.join(incomplete)}"
                    self.store.set_step_state(step_id, state)
                    self.events.put({"type": "step.skipped", "step_id": step_id, "message": state.message})
                    continue
                self._run_one_step(step_id)
        finally:
            self.current_step_id = None
            self.process = None
            self.events.put({"type": "execution.finished", "status": self.status().model_dump()})

    def _run_one_step(self, step_id: str) -> None:
        step = self.store.get_step(step_id)
        self.current_step_id = step_id
        params = self.store.params.get(step_id, {})
        command = self.build_command(step, params)
        cwd = (self.store.current_project() / step.working_directory).resolve()
        log_path = self.store.log_path(step_id)
        state = self.store.state.get(step_id, StepRuntimeState())
        state.status = "running"
        state.exit_code = None
        state.message = ""
        self.store.set_step_state(step_id, state)
        self.events.put({"type": "step.started", "step_id": step_id, "command": command})

        with log_path.open("a", encoding="utf-8") as log_file:
            log_file.write(f"\n[{_now()}] $ {command}\n")
            self.process = subprocess.Popen(
                command,
                cwd=str(cwd),
                shell=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                bufsize=1,
            )
            assert self.process.stdout is not None
            for line in self.process.stdout:
                log_file.write(line)
                log_file.flush()
                self.events.put({"type": "step.log", "step_id": step_id, "line": line.rstrip("\n")})
                if self.stop_requested and self.process.poll() is None:
                    self.process.terminate()
                    break
            exit_code = self.process.wait()

        state.exit_code = exit_code
        state.last_run_at = _now()
        if self.stop_requested:
            state.status = "cancelled"
            state.message = "Execution cancelled by user"
        elif exit_code == 0:
            self.store.done_path(step_id).write_text(_now(), encoding="utf-8")
            if self.store.is_step_ok(step):
                state.status = "ok"
            else:
                state.status = "error"
                state.message = "Command succeeded but expected outputs are missing"
        else:
            state.status = "error"
            state.message = f"Command exited with {exit_code}"
        self.store.set_step_state(step_id, state)
        self.store.validate()
        self.store.save_all()
        self.events.put({"type": "step.finished", "step_id": step_id, "state": state.model_dump()})


def _now() -> str:
    return datetime.now(timezone.utc).isoformat()


def _substitute_placeholders(command: str, values: dict[str, Any]) -> str:
    parts: list[str] = []
    quote_context = "unquoted"
    index = 0
    while index < len(command):
        match = PLACEHOLDER_RE.match(command, index)
        if match:
            parts.append(_quote_placeholder_value(str(values[match.group(1)]), quote_context))
            index = match.end()
            continue

        char = command[index]
        next_char = command[index + 1] if index + 1 < len(command) else ""
        if quote_context == "unquoted":
            if char == "'":
                quote_context = "single"
            elif char == '"':
                quote_context = "double"
            elif char == "\\" and next_char:
                parts.append(command[index : index + 2])
                index += 2
                continue
        elif quote_context == "single":
            if char == "'":
                quote_context = "unquoted"
        elif quote_context == "double":
            if char == "\\" and next_char:
                parts.append(command[index : index + 2])
                index += 2
                continue
            if char == '"':
                quote_context = "unquoted"

        parts.append(char)
        index += 1
    return "".join(parts)


def _quote_placeholder_value(value: str, quote_context: str) -> str:
    if quote_context == "single":
        return value.replace("'", "'\\''")
    if quote_context == "double":
        return "".join(f"\\{char}" if char in {'$', '`', '"', "\\"} else char for char in value)
    return shlex.quote(value)


def format_validation_issue(issue: ValidationIssue) -> str:
    location = issue.step_id or "pipeline"
    if issue.field:
        location = f"{location}.{issue.field}"
    return f"{location}: {issue.message}"


def sse_message(event: dict[str, Any]) -> str:
    return f"data: {json.dumps(event)}\n\n"
