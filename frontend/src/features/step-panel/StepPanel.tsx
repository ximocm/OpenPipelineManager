import { Loader2, Pencil, Play } from 'lucide-react';
import type { PipelineStep, StepDetail, ValueSpec, ValidationIssue } from '../../types/pipeline';

interface StepPanelProps {
  detail?: StepDetail | null;
  onChangeParams: (stepId: string, values: Record<string, unknown>) => void;
  onEditStep: (step: PipelineStep) => void;
  onRunStep: (stepId: string) => void;
  executionRunning: boolean;
}

export function StepPanel({ detail, onChangeParams, onEditStep, onRunStep, executionRunning }: StepPanelProps) {
  if (!detail) {
    return (
      <aside className="detail-panel">
        <div className="panel-title">Step</div>
        <div className="empty">No step selected</div>
      </aside>
    );
  }

  const { step, params, state, validation } = detail;

  return (
    <aside className="detail-panel">
      <div className="panel-title">
        <span>{step.name}</span>
        <div className="panel-actions">
          <button className="icon-button" onClick={() => onEditStep(step)} title="Edit step">
            <Pencil size={16} />
          </button>
          <button className="icon-button" onClick={() => onRunStep(step.id)} disabled={executionRunning} title="Run step">
            {executionRunning ? <Loader2 className="spin" size={16} /> : <Play size={16} />}
          </button>
        </div>
      </div>
      <div className={`state-pill status-${state.status}`}>{state.status}</div>

      <div className="step-meta">
        <div className="step-meta-row">
          <span>Environment</span>
          <code>{step.environment || 'default'}</code>
        </div>
        <div className="step-meta-row">
          <span>Command</span>
          <code>{step.command || 'No command'}</code>
        </div>
      </div>

      <Section title="Inputs" step={step} specs={step.inputs} params={params} issues={validation} onChangeParams={onChangeParams} />
      <Section title="Parameters" step={step} specs={step.parameters} params={params} issues={validation} onChangeParams={onChangeParams} />

      <div className="section">
        <h3>Outputs</h3>
        {step.outputs.length === 0 ? <div className="empty">None</div> : step.outputs.map((output) => <code key={output.path}>{output.path}</code>)}
      </div>

      <div className="section">
        <h3>Validation</h3>
        {validation.length === 0 ? (
          <div className="empty">Clear</div>
        ) : (
          validation.map((issue) => <Issue key={`${issue.field}-${issue.message}`} issue={issue} />)
        )}
      </div>

      {state.message && <div className="inline-error">{state.message}</div>}
    </aside>
  );
}

function Section({
  title,
  step,
  specs,
  params,
  issues,
  onChangeParams,
}: {
  title: string;
  step: PipelineStep;
  specs: ValueSpec[];
  params: Record<string, unknown>;
  issues: ValidationIssue[];
  onChangeParams: (stepId: string, values: Record<string, unknown>) => void;
}) {
  return (
    <div className="section">
      <h3>{title}</h3>
      {specs.length === 0 ? (
        <div className="empty">None</div>
      ) : (
        specs.map((spec) => (
          <FieldEditor
            key={spec.key}
            spec={spec}
            value={params[spec.key] ?? spec.default ?? ''}
            issue={issues.find((item) => item.field === spec.key)}
            onCommit={(value) => onChangeParams(step.id, { [spec.key]: value })}
          />
        ))
      )}
    </div>
  );
}

function FieldEditor({
  spec,
  value,
  issue,
  onCommit,
}: {
  spec: ValueSpec;
  value: unknown;
  issue?: ValidationIssue;
  onCommit: (value: unknown) => void;
}) {
  const label = spec.label || spec.key;
  return (
    <label className="field">
      <span>{label}</span>
      {spec.type === 'boolean' ? (
        <input type="checkbox" checked={Boolean(value)} onChange={(event) => onCommit(event.target.checked)} />
      ) : spec.type === 'select' ? (
        <select value={String(value)} onChange={(event) => onCommit(event.target.value)}>
          {(spec.options ?? []).map((option) => (
            <option key={option} value={option}>
              {option}
            </option>
          ))}
        </select>
      ) : (
        <input
          type={spec.type === 'integer' || spec.type === 'decimal' ? 'number' : 'text'}
          defaultValue={String(value)}
          min={spec.min}
          max={spec.max}
          onBlur={(event) => onCommit(spec.type === 'integer' || spec.type === 'decimal' ? Number(event.target.value) : event.target.value)}
        />
      )}
      {issue && <small className={issue.severity}>{issue.message}</small>}
    </label>
  );
}

function Issue({ issue }: { issue: ValidationIssue }) {
  return <div className={`issue ${issue.severity}`}>{issue.message}</div>;
}
