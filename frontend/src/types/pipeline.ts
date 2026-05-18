export type FieldType = 'text' | 'integer' | 'decimal' | 'boolean' | 'select' | 'file' | 'folder';
export type StepStatus = 'pending' | 'scheduled' | 'running' | 'ok' | 'error' | 'cancelled';
export type IssueSeverity = 'warning' | 'blocker';

export interface ValueSpec {
  key: string;
  label?: string;
  type: FieldType;
  required?: boolean;
  default?: unknown;
  options?: string[];
  min?: number;
  max?: number;
  description?: string;
  source_step?: string;
  source_output?: string;
}

export interface OutputSpec {
  key?: string;
  label?: string;
  path: string;
}

export interface PipelineStep {
  id: string;
  name: string;
  description?: string;
  command?: string;
  environment?: string;
  working_directory?: string;
  inputs: ValueSpec[];
  outputs: OutputSpec[];
  parameters: ValueSpec[];
  dependencies: string[];
}

export interface PipelineConfig {
  steps: PipelineStep[];
}

export interface ValidationIssue {
  severity: IssueSeverity;
  message: string;
  step_id?: string;
  field?: string;
}

export interface StepRuntimeState {
  status: StepStatus;
  selected: boolean;
  exit_code?: number | null;
  last_run_at?: string | null;
  message?: string;
}

export interface ExecutionStatus {
  running: boolean;
  current_step_id?: string | null;
  stop_requested: boolean;
}

export interface ProjectSnapshot {
  project_path: string;
  pipeline?: PipelineConfig | null;
  params: Record<string, Record<string, unknown>>;
  state: Record<string, StepRuntimeState>;
  visual_layout: Record<string, { x: number; y: number }>;
  validation: ValidationIssue[];
}

export interface TreeNode {
  name: string;
  path: string;
  type: 'file' | 'directory';
  children: TreeNode[];
}

export interface FilePreview {
  path: string;
  type: 'text' | 'image' | 'binary' | 'missing';
  content?: string | null;
  media_type?: string | null;
}

export interface DirectoryEntry {
  name: string;
  path: string;
}

export interface DirectoryListing {
  path: string;
  parent?: string | null;
  directories: DirectoryEntry[];
}

export interface FileEntry {
  name: string;
  path: string;
  type: 'file';
}

export interface FileListing {
  path: string;
  parent?: string | null;
  directories: DirectoryEntry[];
  files: FileEntry[];
}

export interface StepDetail {
  step: PipelineStep;
  params: Record<string, unknown>;
  state: StepRuntimeState;
  validation: ValidationIssue[];
}

export interface StepCreatePayload {
  id: string;
  name: string;
  description?: string;
  command?: string;
  environment?: string;
  working_directory?: string;
  inputs?: ValueSpec[];
  outputs?: OutputSpec[];
  parameters?: ValueSpec[];
  dependencies?: string[];
}
