import { apiRequest } from './client';
import type {
  DirectoryListing,
  ExecutionStatus,
  FileListing,
  FilePreview,
  PipelineConfig,
  PipelineStep,
  ProjectSnapshot,
  StepCreatePayload,
  StepDetail,
  TreeNode,
  ValidationIssue,
} from '../types/pipeline';

export const getCurrentProject = () => apiRequest<ProjectSnapshot>('/api/projects/current');
export const createProject = (path: string) =>
  apiRequest<ProjectSnapshot>('/api/projects', { method: 'POST', body: JSON.stringify({ path }) });
export const openProject = (path: string) =>
  apiRequest<ProjectSnapshot>('/api/projects/open', { method: 'POST', body: JSON.stringify({ path }) });
export const getProjectTree = () => apiRequest<TreeNode>('/api/projects/tree');
export const browseDirectories = (path?: string) =>
  apiRequest<DirectoryListing>(`/api/filesystem/directories${path ? `?path=${encodeURIComponent(path)}` : ''}`);
export const browseFiles = (path?: string, extensions?: string[]) => {
  const params = new URLSearchParams();
  if (path) params.set('path', path);
  if (extensions?.length) params.set('extensions', extensions.join(','));
  const query = params.toString();
  return apiRequest<FileListing>(`/api/filesystem/files${query ? `?${query}` : ''}`);
};
export const createFilePath = (path: string, directory = false, content = '') =>
  apiRequest<{ path: string }>('/api/projects/files', { method: 'POST', body: JSON.stringify({ path, directory, content }) });
export const updateFileContent = (path: string, content: string) =>
  apiRequest<{ path: string }>('/api/projects/files', { method: 'PUT', body: JSON.stringify({ path, content }) });
export const renameFilePath = (path: string, name: string) =>
  apiRequest<{ path: string }>('/api/projects/files/rename', { method: 'PATCH', body: JSON.stringify({ path, name }) });
export const moveFilePath = (source: string, targetDirectory: string) =>
  apiRequest<{ path: string }>('/api/projects/files/move', {
    method: 'POST',
    body: JSON.stringify({ source, target_directory: targetDirectory }),
  });
export const deleteFilePath = (path: string) =>
  apiRequest<{ path: string }>('/api/projects/files/delete', { method: 'POST', body: JSON.stringify({ path }) });
export const uploadFilePayloads = (targetDirectory: string, files: Array<{ name: string; content_base64: string }>) =>
  apiRequest<{ paths: string[] }>('/api/projects/files/upload', {
    method: 'POST',
    body: JSON.stringify({ target_directory: targetDirectory, files }),
  });
export const getFilePreview = (path: string) => apiRequest<FilePreview>(`/api/files/preview?path=${encodeURIComponent(path)}`);
export const importPipeline = (path: string) =>
  apiRequest<PipelineConfig>('/api/pipeline/import', { method: 'POST', body: JSON.stringify({ path }) });
export const getValidation = () => apiRequest<ValidationIssue[]>('/api/pipeline/validation');
export const getStep = (stepId: string) => apiRequest<StepDetail>(`/api/steps/${encodeURIComponent(stepId)}`);
export const updateStepParams = (stepId: string, values: Record<string, unknown>) =>
  apiRequest(`/api/steps/${encodeURIComponent(stepId)}/params`, { method: 'POST', body: JSON.stringify({ values }) });
export const updateStepSelection = (stepId: string, selected: boolean) =>
  apiRequest(`/api/steps/${encodeURIComponent(stepId)}/selection`, { method: 'POST', body: JSON.stringify({ selected }) });
export const runStep = (stepId: string) =>
  apiRequest(`/api/steps/${encodeURIComponent(stepId)}/run`, { method: 'POST' });
export const runSelected = (stepIds?: string[]) =>
  apiRequest('/api/steps/run-selected', { method: 'POST', body: JSON.stringify({ step_ids: stepIds }) });
export const stopExecution = () => apiRequest('/api/steps/stop', { method: 'POST' });
export const getExecutionStatus = () => apiRequest<ExecutionStatus>('/api/execution/status');
export const saveLayout = (positions: Record<string, { x: number; y: number }>) =>
  apiRequest('/api/pipeline/layout', { method: 'POST', body: JSON.stringify({ positions }) });
export const createPipelineStep = (step: StepCreatePayload) =>
  apiRequest<PipelineStep>('/api/pipeline/steps', { method: 'POST', body: JSON.stringify(step) });
export const updatePipelineStep = (stepId: string, step: StepCreatePayload) =>
  apiRequest<PipelineStep>(`/api/pipeline/steps/${encodeURIComponent(stepId)}`, {
    method: 'PUT',
    body: JSON.stringify(step),
  });
export const updateStepDependencies = (stepId: string, dependencies: string[]) =>
  apiRequest(`/api/pipeline/steps/${encodeURIComponent(stepId)}/dependencies`, {
    method: 'PATCH',
    body: JSON.stringify({ dependencies }),
  });
export const getLogs = (stepId: string) => apiRequest<{ step_id: string; content: string }>(`/api/logs/${stepId}`);
