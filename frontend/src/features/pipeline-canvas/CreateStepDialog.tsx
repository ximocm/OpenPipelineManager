import { Plus, Trash2, X } from 'lucide-react';
import { useMemo, useRef, useState, type FormEvent } from 'react';
import type { FieldType, PipelineStep, StepCreatePayload, ValueSpec } from '../../types/pipeline';

interface CreateStepDialogProps {
  suggestedIndex: number;
  initialStep?: PipelineStep | null;
  mode?: 'create' | 'edit';
  pipelineSteps?: PipelineStep[];
  onCreate: (step: StepCreatePayload) => void;
  onClose: () => void;
}

interface ValueDraft {
  id: number;
  key: string;
  label: string;
  type: FieldType;
  required: boolean;
  defaultValue: string;
  choices: string;
  minimum: string;
  maximum: string;
  sourceStep: string;
  sourceOutput: string;
}

interface OutputDraft {
  id: number;
  key: string;
  label: string;
  path: string;
}

interface OutputChoice {
  id: string;
  label: string;
  stepId: string;
  outputKey: string;
  outputPath: string;
  defaultValue: string;
}

const fieldTypes: FieldType[] = ['text', 'integer', 'decimal', 'boolean', 'select', 'file', 'folder'];
const fieldTypeLabels: Record<FieldType, string> = {
  text: 'Text',
  integer: 'Integer',
  decimal: 'Decimal',
  boolean: 'Boolean',
  select: 'Selector (predefined options)',
  file: 'File',
  folder: 'Folder',
};

export function CreateStepDialog({
  suggestedIndex,
  initialStep,
  mode = 'create',
  pipelineSteps = [],
  onCreate,
  onClose,
}: CreateStepDialogProps) {
  const defaultId = `step_${suggestedIndex + 1}`;
  const [stepId, setStepId] = useState(initialStep?.id ?? defaultId);
  const [name, setName] = useState(initialStep?.name ?? `Step ${suggestedIndex + 1}`);
  const [command, setCommand] = useState(initialStep?.command ?? '');
  const [environment, setEnvironment] = useState(initialStep?.environment ?? '');
  const [workingDirectory, setWorkingDirectory] = useState(initialStep?.working_directory ?? '');
  const [inputs, setInputs] = useState<ValueDraft[]>(() => (initialStep?.inputs ?? []).map(toValueDraft));
  const [options, setOptions] = useState<ValueDraft[]>(() => (initialStep?.parameters ?? []).map(toValueDraft));
  const [outputs, setOutputs] = useState<OutputDraft[]>(() => (initialStep?.outputs ?? []).map(toOutputDraft));
  const [dependencies, setDependencies] = useState<string[]>(initialStep?.dependencies ?? []);
  const commandInputRef = useRef<HTMLTextAreaElement>(null);

  const cleanStepId = useMemo(() => sanitizeIdentifier(stepId) || defaultId, [stepId, defaultId]);
  const resolvedWorkingDirectory = workingDirectory.trim() || `steps/${cleanStepId}/work`;
  const isEditing = mode === 'edit';
  const commandKeywords = useMemo(
    () =>
      Array.from(
        new Set(
          [...inputs, ...options]
            .map((value) => sanitizeIdentifier(value.key))
            .concat(outputs.map((output) => sanitizeIdentifier(output.key)))
            .filter(Boolean),
        ),
      ),
    [inputs, options, outputs],
  );
  const previousOutputChoices = useMemo(
    () => buildPreviousOutputChoices(pipelineSteps, initialStep?.id ?? null, resolvedWorkingDirectory),
    [pipelineSteps, initialStep?.id, resolvedWorkingDirectory],
  );

  const handleSubmit = (event: FormEvent) => {
    event.preventDefault();
    onCreate({
      id: cleanStepId,
      name: name.trim() || cleanStepId,
      command: command.trim(),
      environment: environment.trim(),
      working_directory: resolvedWorkingDirectory,
      inputs: inputs.map(toValueSpec).filter((spec): spec is ValueSpec => Boolean(spec)),
      outputs: outputs.map(toOutputSpec).filter((output): output is { path: string } => Boolean(output)),
      parameters: options.map(toValueSpec).filter((spec): spec is ValueSpec => Boolean(spec)),
      dependencies,
    });
  };

  const addInput = () => setInputs((current) => [...current, emptyValueDraft('file')]);
  const addOption = () => setOptions((current) => [...current, emptyValueDraft('text')]);
  const addOutput = () => setOutputs((current) => [...current, { id: Date.now(), key: '', label: '', path: '' }]);
  const updateInput = (id: number, patch: Partial<ValueDraft>) => {
    setInputs((current) => current.map((input) => (input.id === id ? { ...input, ...patch } : input)));
  };
  const updateOption = (id: number, patch: Partial<ValueDraft>) => {
    setOptions((current) => current.map((option) => (option.id === id ? { ...option, ...patch } : option)));
  };
  const updateOutput = (id: number, patch: Partial<OutputDraft>) => {
    setOutputs((current) => current.map((output) => (output.id === id ? { ...output, ...patch } : output)));
  };
  const usePreviousOutput = (inputId: number, choiceId: string) => {
    const choice = previousOutputChoices.find((item) => item.id === choiceId);
    if (!choice) return;
    updateInput(inputId, {
      defaultValue: choice.defaultValue,
      sourceStep: choice.stepId,
      sourceOutput: choice.outputKey || choice.outputPath,
    });
    setDependencies((current) => (current.includes(choice.stepId) ? current : [...current, choice.stepId]));
  };
  const insertCommandText = (text: string) => {
    const input = commandInputRef.current;
    if (!input) {
      setCommand((current) => `${current}${current && !/\s$/.test(current) ? ' ' : ''}${text}`);
      return;
    }

    const start = input.selectionStart ?? command.length;
    const end = input.selectionEnd ?? command.length;
    const nextCommand = `${command.slice(0, start)}${text}${command.slice(end)}`;
    setCommand(nextCommand);
    requestAnimationFrame(() => {
      input.focus();
      input.setSelectionRange(start + text.length, start + text.length);
    });
  };
  const insertCommandKeyword = (keyword: string) => insertCommandText(`{${keyword}}`);
  const insertPipelineVariable = (key: string) => {
    const cleanKey = sanitizeIdentifier(key);
    if (!cleanKey) return;
    insertCommandText(`${toEnvironmentVariable(cleanKey)}={${cleanKey}}`);
  };

  return (
    <div className="modal-backdrop">
      <form className="step-dialog" onSubmit={handleSubmit}>
        <div className="dialog-header">
          <span>{isEditing ? 'Edit step' : 'Create step'}</span>
          <button type="button" className="icon-button" onClick={onClose} title="Close">
            <X size={16} />
          </button>
        </div>

        <div className="step-form-grid">
          <label className="field">
            <span>Step id</span>
            <input value={stepId} onChange={(event) => setStepId(event.target.value)} required />
          </label>
          <label className="field">
            <span>Name</span>
            <input value={name} onChange={(event) => setName(event.target.value)} required />
          </label>
          <label className="field">
            <span>Environment</span>
            <input value={environment} onChange={(event) => setEnvironment(event.target.value)} placeholder="conda:analysis" />
          </label>
          <label className="field">
            <span>Working directory</span>
            <input
              value={workingDirectory}
              onChange={(event) => setWorkingDirectory(event.target.value)}
              placeholder={resolvedWorkingDirectory}
            />
          </label>
          <div className="field field-wide">
            <span>Command</span>
            <textarea
              className="command-textarea"
              ref={commandInputRef}
              value={command}
              onChange={(event) => setCommand(event.target.value)}
              placeholder="python script.py --mode {mode}"
              rows={3}
            />
            {commandKeywords.length > 0 && (
              <div className="keyword-row">
                {commandKeywords.map((keyword) => (
                  <button key={keyword} type="button" className="keyword-button" onClick={() => insertCommandKeyword(keyword)}>
                    {`{${keyword}}`}
                  </button>
                ))}
              </div>
            )}
          </div>
        </div>

        <div className="step-dialog-scroll">
          <ValueSpecSection
            title="Inputs"
            addLabel="Add input"
            values={inputs}
            outputChoices={previousOutputChoices}
            onAdd={addInput}
            onUpdate={updateInput}
            onUseOutput={usePreviousOutput}
            onInsertVariable={insertPipelineVariable}
            onRemove={(id) => setInputs((current) => current.filter((item) => item.id !== id))}
          />
          <ValueSpecSection
            title="Options"
            addLabel="Add option"
            values={options}
            onAdd={addOption}
            onUpdate={updateOption}
            onInsertVariable={insertPipelineVariable}
            onRemove={(id) => setOptions((current) => current.filter((item) => item.id !== id))}
          />
          <OutputSection
            outputs={outputs}
            onAdd={addOutput}
            onUpdate={updateOutput}
            onInsertVariable={insertPipelineVariable}
            onRemove={(id) => setOutputs((current) => current.filter((item) => item.id !== id))}
          />
        </div>

        <div className="dialog-footer">
          <span>{resolvedWorkingDirectory}</span>
          <button type="submit" className="command-button">
            {isEditing ? 'Save step' : 'Create step'}
          </button>
        </div>
      </form>
    </div>
  );
}

function ValueSpecSection({
  title,
  addLabel,
  values,
  onAdd,
  onUpdate,
  onUseOutput,
  onInsertVariable,
  onRemove,
  outputChoices = [],
}: {
  title: string;
  addLabel: string;
  values: ValueDraft[];
  onAdd: () => void;
  onUpdate: (id: number, patch: Partial<ValueDraft>) => void;
  onUseOutput?: (id: number, choiceId: string) => void;
  onInsertVariable: (key: string) => void;
  onRemove: (id: number) => void;
  outputChoices?: OutputChoice[];
}) {
  return (
    <div className="step-config-section">
      <div className="step-options-header">
        <span>{title}</span>
        <button type="button" className="command-button" onClick={onAdd}>
          <Plus size={15} />
          {addLabel}
        </button>
      </div>

      <div className="step-options-list">
        {values.length === 0 ? (
          <div className="empty">None</div>
        ) : (
          values.map((value) => (
            <div className="step-option-row" key={value.id}>
              <label className="field">
                <span>Key</span>
                <input value={value.key} onChange={(event) => onUpdate(value.id, { key: event.target.value })} />
              </label>
              <label className="field">
                <span>Label</span>
                <input value={value.label} onChange={(event) => onUpdate(value.id, { label: event.target.value })} />
              </label>
              <label className="field">
                <span>Type</span>
                <select value={value.type} onChange={(event) => onUpdate(value.id, { type: event.target.value as FieldType })}>
                  {fieldTypes.map((type) => (
                    <option key={type} value={type}>
                      {fieldTypeLabels[type]}
                    </option>
                  ))}
                </select>
              </label>
              <label className="field">
                <span>Default</span>
                <input value={value.defaultValue} onChange={(event) => onUpdate(value.id, { defaultValue: event.target.value })} />
              </label>
              <button
                type="button"
                className="command-button pipeline-variable-button"
                disabled={!sanitizeIdentifier(value.key)}
                onClick={() => onInsertVariable(value.key)}
                title="Insert VARIABLE={key} in command"
              >
                Pipeline var
              </button>
              {value.type === 'select' && (
                <label className="field field-wide">
                  <span>Predefined options</span>
                  <input
                    value={value.choices}
                    onChange={(event) => onUpdate(value.id, { choices: event.target.value })}
                    placeholder="fast, accurate, custom"
                  />
                </label>
              )}
              {onUseOutput && (value.type === 'file' || value.type === 'folder') && (
                <label className="field field-wide">
                  <span>From output</span>
                  <select defaultValue="" onChange={(event) => onUseOutput(value.id, event.target.value)}>
                    <option value="">Select previous output</option>
                    {outputChoices.map((choice) => (
                      <option key={choice.id} value={choice.id}>
                        {choice.label}
                      </option>
                    ))}
                  </select>
                </label>
              )}
              {(value.type === 'integer' || value.type === 'decimal') && (
                <>
                  <label className="field">
                    <span>Min</span>
                    <input value={value.minimum} onChange={(event) => onUpdate(value.id, { minimum: event.target.value })} />
                  </label>
                  <label className="field">
                    <span>Max</span>
                    <input value={value.maximum} onChange={(event) => onUpdate(value.id, { maximum: event.target.value })} />
                  </label>
                </>
              )}
              <label className="field checkbox-field">
                <span>Required</span>
                <input
                  type="checkbox"
                  checked={value.required}
                  onChange={(event) => onUpdate(value.id, { required: event.target.checked })}
                />
              </label>
              <button type="button" className="icon-button danger-button" onClick={() => onRemove(value.id)} title={`Remove ${title.toLowerCase()}`}>
                <Trash2 size={15} />
              </button>
            </div>
          ))
        )}
      </div>
    </div>
  );
}

function OutputSection({
  outputs,
  onAdd,
  onUpdate,
  onInsertVariable,
  onRemove,
}: {
  outputs: OutputDraft[];
  onAdd: () => void;
  onUpdate: (id: number, patch: Partial<OutputDraft>) => void;
  onInsertVariable: (key: string) => void;
  onRemove: (id: number) => void;
}) {
  return (
    <div className="step-config-section">
      <div className="step-options-header">
        <span>Outputs</span>
        <button type="button" className="command-button" onClick={onAdd}>
          <Plus size={15} />
          Add output
        </button>
      </div>
      <div className="step-options-list">
        {outputs.length === 0 ? (
          <div className="empty">None</div>
        ) : (
          outputs.map((output) => (
            <div className="step-output-row" key={output.id}>
              <label className="field">
                <span>Key</span>
                <input value={output.key} onChange={(event) => onUpdate(output.id, { key: event.target.value })} placeholder="result_file" />
              </label>
              <label className="field">
                <span>Label</span>
                <input value={output.label} onChange={(event) => onUpdate(output.id, { label: event.target.value })} placeholder="Result file" />
              </label>
              <label className="field">
                <span>Path</span>
                <input value={output.path} onChange={(event) => onUpdate(output.id, { path: event.target.value })} placeholder="outputs/result.txt" />
              </label>
              <button
                type="button"
                className="command-button pipeline-variable-button"
                disabled={!sanitizeIdentifier(output.key)}
                onClick={() => onInsertVariable(output.key)}
                title="Insert VARIABLE={key} in command"
              >
                Pipeline var
              </button>
              <button type="button" className="icon-button danger-button" onClick={() => onRemove(output.id)} title="Remove output">
                <Trash2 size={15} />
              </button>
            </div>
          ))
        )}
      </div>
    </div>
  );
}

function emptyValueDraft(type: FieldType): ValueDraft {
  return {
    id: Date.now(),
    key: '',
    label: '',
    type,
    required: false,
    defaultValue: '',
    choices: '',
    minimum: '',
    maximum: '',
    sourceStep: '',
    sourceOutput: '',
  };
}

function toValueDraft(spec: ValueSpec, index: number): ValueDraft {
  return {
    id: Date.now() + index,
    key: spec.key,
    label: spec.label ?? '',
    type: spec.type,
    required: Boolean(spec.required),
    defaultValue: spec.default === undefined || spec.default === null ? '' : String(spec.default),
    choices: (spec.options ?? []).join(', '),
    minimum: spec.min === undefined || spec.min === null ? '' : String(spec.min),
    maximum: spec.max === undefined || spec.max === null ? '' : String(spec.max),
    sourceStep: spec.source_step ?? '',
    sourceOutput: spec.source_output ?? '',
  };
}

function toOutputDraft(output: { path: string }, index: number): OutputDraft {
  return {
    id: Date.now() + index,
    key: 'key' in output && typeof output.key === 'string' ? output.key : '',
    label: 'label' in output && typeof output.label === 'string' ? output.label : '',
    path: output.path,
  };
}

function toValueSpec(option: ValueDraft): ValueSpec | null {
  const key = sanitizeIdentifier(option.key);
  if (!key) return null;

  const spec: ValueSpec = {
    key,
    label: option.label.trim() || key,
    type: option.type,
    required: option.required,
  };
  const defaultValue = parseDefaultValue(option.type, option.defaultValue);
  if (defaultValue !== undefined) spec.default = defaultValue;

  if (option.type === 'select') {
    spec.options = option.choices
      .split(',')
      .map((choice) => choice.trim())
      .filter(Boolean);
  }
  const minimum = Number(option.minimum);
  const maximum = Number(option.maximum);
  if ((option.type === 'integer' || option.type === 'decimal') && option.minimum.trim() && Number.isFinite(minimum)) spec.min = minimum;
  if ((option.type === 'integer' || option.type === 'decimal') && option.maximum.trim() && Number.isFinite(maximum)) spec.max = maximum;
  if (option.sourceStep && option.sourceOutput) {
    spec.source_step = option.sourceStep;
    spec.source_output = option.sourceOutput;
  }

  return spec;
}

function toOutputSpec(output: OutputDraft): { key?: string; label?: string; path: string } | null {
  const path = output.path.trim();
  if (!path) return null;
  const key = sanitizeIdentifier(output.key);
  const label = output.label.trim();
  return {
    ...(key ? { key } : {}),
    ...(label ? { label } : {}),
    path,
  };
}

function buildPreviousOutputChoices(
  pipelineSteps: PipelineStep[],
  currentStepId: string | null,
  targetWorkingDirectory: string,
): OutputChoice[] {
  const currentIndex = currentStepId ? pipelineSteps.findIndex((step) => step.id === currentStepId) : pipelineSteps.length;
  const previousSteps = pipelineSteps.slice(0, currentIndex < 0 ? pipelineSteps.length : currentIndex);

  return previousSteps.flatMap((step) =>
    step.outputs.map((output, index) => {
      const outputProjectPath = joinPath(step.working_directory || '.', output.path);
      const defaultValue = relativePath(targetWorkingDirectory || '.', outputProjectPath);
      return {
        id: `${step.id}:${index}:${output.path}`,
        label: `${step.id} -> ${output.path}`,
        stepId: step.id,
        outputKey: output.key ?? '',
        outputPath: output.path,
        defaultValue,
      };
    }),
  );
}

function joinPath(...parts: string[]): string {
  return normalizePath(parts.filter(Boolean).join('/'));
}

function normalizePath(path: string): string {
  const segments: string[] = [];
  path
    .replace(/\\/g, '/')
    .split('/')
    .forEach((segment) => {
      if (!segment || segment === '.') return;
      if (segment === '..') {
        if (segments.length && segments[segments.length - 1] !== '..') segments.pop();
        else segments.push(segment);
        return;
      }
      segments.push(segment);
    });
  return segments.join('/') || '.';
}

function relativePath(fromDirectory: string, toPath: string): string {
  const from = normalizePath(fromDirectory).split('/').filter((segment) => segment !== '.');
  const to = normalizePath(toPath).split('/').filter((segment) => segment !== '.');

  let shared = 0;
  while (shared < from.length && shared < to.length && from[shared] === to[shared]) {
    shared += 1;
  }

  const upward = from.slice(shared).map(() => '..');
  const downward = to.slice(shared);
  return [...upward, ...downward].join('/') || '.';
}

function parseDefaultValue(type: FieldType, value: string): unknown {
  const cleanValue = value.trim();
  if (!cleanValue) return undefined;
  if (type === 'boolean') return ['1', 'true', 'yes', 'on'].includes(cleanValue.toLowerCase());
  if (type === 'integer') {
    const parsed = Number.parseInt(cleanValue, 10);
    return Number.isFinite(parsed) ? parsed : undefined;
  }
  if (type === 'decimal') {
    const parsed = Number(cleanValue);
    return Number.isFinite(parsed) ? parsed : undefined;
  }
  return cleanValue;
}

function sanitizeIdentifier(value: string): string {
  return value
    .trim()
    .replace(/\s+/g, '_')
    .replace(/[^\w.-]/g, '_')
    .replace(/_+/g, '_')
    .replace(/^[._]+|[._]+$/g, '');
}

function toEnvironmentVariable(key: string): string {
  const variable = key.replace(/[^A-Za-z0-9_]/g, '_').replace(/_+/g, '_').toUpperCase();
  return /^[0-9]/.test(variable) ? `VAR_${variable}` : variable;
}
