import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { AlertTriangle, CheckCircle2, CircleStop, Loader2, Play, Plus, Save, X } from 'lucide-react';
import { apiUrl } from './api/client';
import {
  browseDirectories,
  browseFiles,
  createFilePath,
  createPipelineStep,
  createProject,
  deleteFilePath,
  getFilePreview,
  getCurrentProject,
  getExecutionStatus,
  getLogs,
  getProjectTree,
  getStep,
  getValidation,
  importPipeline,
  moveFilePath,
  openProject,
  renameFilePath,
  runSelected,
  runStep,
  saveLayout,
  stopExecution,
  uploadFilePayloads,
  updateFileContent,
  updatePipelineStep,
  updateStepDependencies,
  updateStepParams,
  updateStepSelection,
} from './api/pipeline';
import { LogsPanel } from './features/logs-panel/LogsPanel';
import { CreateStepDialog } from './features/pipeline-canvas/CreateStepDialog';
import { PipelineCanvas } from './features/pipeline-canvas/PipelineCanvas';
import { DirectoryPicker, ProjectTree } from './features/project-tree/ProjectTree';
import { StepPanel } from './features/step-panel/StepPanel';
import type {
  DirectoryListing,
  FileListing,
  FilePreview,
  PipelineStep,
  ProjectSnapshot,
  StepCreatePayload,
  StepDetail,
  TreeNode,
  ValidationIssue,
} from './types/pipeline';

interface FileTab {
  path: string;
  preview: FilePreview;
  draft: string;
  dirty: boolean;
  saving: boolean;
}

export function App() {
  const [projectPath, setProjectPath] = useState('');
  const [pipelinePath, setPipelinePath] = useState('');
  const [snapshot, setSnapshot] = useState<ProjectSnapshot | null>(null);
  const [tree, setTree] = useState<TreeNode | null>(null);
  const [validation, setValidation] = useState<ValidationIssue[]>([]);
  const [selectedStepId, setSelectedStepId] = useState<string | null>(null);
  const [detail, setDetail] = useState<StepDetail | null>(null);
  const [directoryListing, setDirectoryListing] = useState<DirectoryListing | null>(null);
  const [directoryPickerOpen, setDirectoryPickerOpen] = useState(false);
  const [directoryPickerMode, setDirectoryPickerMode] = useState<'create' | 'open'>('open');
  const [fileListing, setFileListing] = useState<FileListing | null>(null);
  const [filePickerOpen, setFilePickerOpen] = useState(false);
  const [stepDialogOpen, setStepDialogOpen] = useState(false);
  const [editingStep, setEditingStep] = useState<PipelineStep | null>(null);
  const [logs, setLogs] = useState('');
  const [executionRunning, setExecutionRunning] = useState(false);
  const [fileTabs, setFileTabs] = useState<FileTab[]>([]);
  const [activeCenterTab, setActiveCenterTab] = useState('pipeline');
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const restoredSession = useRef(false);

  const refresh = useCallback(async () => {
    const [project, executionStatus] = await Promise.all([getCurrentProject(), getExecutionStatus()]);
    setExecutionRunning(executionStatus.running);
    setSnapshot(project);
    setProjectPath(project.project_path);
    if (!project.project_path) {
      setTree(null);
      setValidation([]);
      setSelectedStepId(null);
      setDetail(null);
      setLogs('');
      return;
    }

    const [projectTree, issues] = await Promise.all([getProjectTree(), getValidation()]);
    setTree(projectTree);
    setValidation(issues);
    const selectedStepExists = Boolean(project.pipeline?.steps.some((step) => step.id === selectedStepId));
    if (selectedStepId && selectedStepExists) {
      setDetail(await getStep(selectedStepId));
      const logResponse = await getLogs(selectedStepId);
      setLogs(logResponse.content);
    } else if (selectedStepId) {
      setSelectedStepId(null);
      setDetail(null);
      setLogs('');
    }
  }, [selectedStepId]);

  useEffect(() => {
    if (restoredSession.current) return;
    restoredSession.current = true;

    const restore = async () => {
      const saved = loadStoredSession();
      if (saved?.projectPath) {
        try {
          const project = await openProject(saved.projectPath);
          setProjectPath(project.project_path);
          if (saved.pipelinePath) setPipelinePath(saved.pipelinePath);
          saveStoredSession(project.project_path, saved.pipelinePath);
          await refresh();
          return;
        } catch (err) {
          clearStoredSession();
          setError(err instanceof Error ? err.message : String(err));
        }
      }
      await refresh();
    };

    restore().catch((err: Error) => setError(err.message));
  }, [refresh]);

  useEffect(() => {
    const source = new EventSource(apiUrl('/api/execution/events'));
    source.onmessage = (event) => {
      const payload = JSON.parse(event.data) as { type: string; step_id?: string; line?: string; status?: { running?: boolean } };
      if (payload.type === 'step.started') {
        setExecutionRunning(true);
      }
      if (payload.type === 'step.log' && payload.step_id === selectedStepId && payload.line) {
        setLogs((current) => `${current}${current ? '\n' : ''}${payload.line}`);
      }
      if (payload.type === 'step.finished' || payload.type === 'execution.finished') {
        if (payload.type === 'execution.finished') setExecutionRunning(Boolean(payload.status?.running));
        refresh().catch((err: Error) => setError(err.message));
      }
    };
    return () => source.close();
  }, [refresh, selectedStepId]);

  const selectedIssues = useMemo(
    () => validation.filter((issue) => issue.step_id === selectedStepId),
    [validation, selectedStepId],
  );
  const blockers = validation.filter((issue) => issue.severity === 'blocker').length;
  const warnings = validation.filter((issue) => issue.severity === 'warning').length;

  const handleShowCreateProjectPicker = async () => {
    setDirectoryPickerMode('create');
    setDirectoryPickerOpen(true);
    await runAction('', async () => {
      setDirectoryListing(await browseDirectories(projectPath || undefined));
      return '';
    });
  };

  const handleShowOpenProjectPicker = async () => {
    setDirectoryPickerMode('open');
    setDirectoryPickerOpen(true);
    await runAction('', async () => {
      setDirectoryListing(await browseDirectories(projectPath || undefined));
      return '';
    });
  };

  const handleBrowseDirectory = async (path?: string) => {
    await runAction('', async () => {
      setDirectoryListing(await browseDirectories(path));
      return '';
    });
  };

  const handleShowPipelinePicker = async () => {
    setFilePickerOpen(true);
    await runAction('', async () => {
      setFileListing(await browseFiles(projectPath || undefined, ['.yml', '.yaml', '.json']));
      return '';
    });
  };

  const handleBrowsePipelineFiles = async (path?: string) => {
    await runAction('', async () => {
      setFileListing(await browseFiles(path, ['.yml', '.yaml', '.json']));
      return '';
    });
  };

  const handleSelectPipelineFile = async (path: string) => {
    await runAction('Pipeline imported', async () => {
      setPipelinePath(path);
      setFilePickerOpen(false);
      await importPipeline(path);
      if (projectPath) saveStoredSession(projectPath, path);
      await refresh();
      return displayName(path);
    });
  };

  const handleOpenProject = async (path: string) => {
    await runAction('Project opened', async () => {
      const project = await openProject(path);
      setProjectPath(project.project_path);
      setPipelinePath('');
      setDirectoryPickerOpen(false);
      saveStoredSession(project.project_path);
      await refresh();
      return displayName(project.project_path);
    });
  };

  const handleCreateProject = async (parentPath: string, projectName: string) => {
    await runAction('Project created', async () => {
      const project = await createProject(joinFilesystemPath(parentPath, projectName));
      setProjectPath(project.project_path);
      setPipelinePath('');
      setDirectoryPickerOpen(false);
      saveStoredSession(project.project_path);
      await refresh();
      return displayName(project.project_path);
    });
  };

  const handleImportPipeline = async () => {
    await runAction('Pipeline imported', async () => {
      await importPipeline(pipelinePath);
      if (projectPath) saveStoredSession(projectPath, pipelinePath);
      await refresh();
      return displayName(pipelinePath);
    });
  };

  const handleSelectStep = useCallback(async (stepId: string) => {
    setSelectedStepId(stepId);
    setDetail(await getStep(stepId));
    const logResponse = await getLogs(stepId);
    setLogs(logResponse.content);
  }, []);

  const handleParams = async (stepId: string, values: Record<string, unknown>) => {
    await updateStepParams(stepId, values);
    setDetail(await getStep(stepId));
    setValidation(await getValidation());
  };

  const handleToggleStep = useCallback(
    async (stepId: string, selected: boolean) => {
      await updateStepSelection(stepId, selected);
      await refresh();
    },
    [refresh],
  );

  const handleRunStep = useCallback(
    async (stepId: string) => {
      if (executionRunning) return;
      setLogs('');
      setExecutionRunning(true);
      try {
        await runStep(stepId);
        await refresh();
      } catch (err) {
        setExecutionRunning(false);
        throw err;
      }
    },
    [executionRunning, refresh],
  );

  const handleRunSelected = async () => {
    if (executionRunning) return;
    setLogs('');
    setExecutionRunning(true);
    try {
      await runSelected();
      await refresh();
    } catch (err) {
      setExecutionRunning(false);
      throw err;
    }
  };

  const handleStop = async () => {
    await stopExecution();
    setExecutionRunning(false);
    await refresh();
  };

  const handleCreateStep = async (step: StepCreatePayload) => {
    await runAction('Step created', async () => {
      const created = await createPipelineStep(step);
      setStepDialogOpen(false);
      setEditingStep(null);
      await refresh();
      setSelectedStepId(created.id);
      setDetail(await getStep(created.id));
      return created.id;
    });
  };

  const handleEditStep = async (step: StepCreatePayload) => {
    if (!editingStep) return;
    await runAction('Step saved', async () => {
      const updated = await updatePipelineStep(editingStep.id, step);
      setStepDialogOpen(false);
      setEditingStep(null);
      await refresh();
      setSelectedStepId(updated.id);
      setDetail(await getStep(updated.id));
      return updated.id;
    });
  };

  const handleConnectSteps = useCallback(
    async (source: string, target: string) => {
      await runAction('', async () => {
        const targetStep = snapshot?.pipeline?.steps.find((step) => step.id === target);
        if (!targetStep) throw new Error(`Unknown step: ${target}`);
        const dependencies = Array.from(new Set([...(targetStep.dependencies ?? []), source]));
        await updateStepDependencies(target, dependencies);
        await refresh();
        return '';
      });
    },
    [refresh, snapshot],
  );

  const handleDisconnectSteps = useCallback(
    async (edges: Array<{ source: string; target: string }>) => {
      await runAction('', async () => {
        const pipeline = snapshot?.pipeline;
        if (!pipeline || edges.length === 0) return '';

        const dependenciesByStep = new Map(pipeline.steps.map((step) => [step.id, new Set(step.dependencies ?? [])]));
        const touched = new Set<string>();
        edges.forEach((edge) => {
          dependenciesByStep.get(edge.target)?.delete(edge.source);
          touched.add(edge.target);
        });

        await Promise.all(
          [...touched]
            .filter((stepId) => dependenciesByStep.has(stepId))
            .map((stepId) => updateStepDependencies(stepId, [...(dependenciesByStep.get(stepId) ?? [])])),
        );
        await refresh();
        return '';
      });
    },
    [refresh, snapshot],
  );

  const handleReconnectStep = useCallback(
    async (edge: { source: string; target: string }, connection: { source?: string | null; target?: string | null }) => {
      await runAction('', async () => {
        const pipeline = snapshot?.pipeline;
        if (!pipeline || !connection.source || !connection.target || connection.source === connection.target) return '';

        const dependenciesByStep = new Map(pipeline.steps.map((step) => [step.id, new Set(step.dependencies ?? [])]));
        dependenciesByStep.get(edge.target)?.delete(edge.source);
        dependenciesByStep.get(connection.target)?.add(connection.source);

        const touched = new Set([edge.target, connection.target]);
        await Promise.all(
          [...touched]
            .filter((stepId) => dependenciesByStep.has(stepId))
            .map((stepId) => updateStepDependencies(stepId, [...(dependenciesByStep.get(stepId) ?? [])])),
        );
        await refresh();
        return '';
      });
    },
    [refresh, snapshot],
  );

  const handleCreatePath = async (path: string, directory: boolean) => {
    await runAction(directory ? 'Folder created' : 'File created', async () => {
      await createFilePath(path, directory, directory ? '' : '');
      await refresh();
      return path;
    });
  };

  const handleRenamePath = async (path: string, name: string) => {
    await runAction('Renamed', async () => {
      const response = await renameFilePath(path, name);
      setFileTabs((current) =>
        current.map((tab) => {
          const nextPath = replacePathPrefix(tab.path, path, response.path);
          return nextPath ? { ...tab, path: nextPath, preview: { ...tab.preview, path: nextPath } } : tab;
        }),
      );
      if (activeCenterTab.startsWith('file:')) {
        const activePath = activeCenterTab.slice('file:'.length);
        const nextActivePath = replacePathPrefix(activePath, path, response.path);
        if (nextActivePath) setActiveCenterTab(fileTabId(nextActivePath));
      }
      await refresh();
      return response.path;
    });
  };

  const handleMovePath = async (source: string, targetDirectory: string) => {
    await runAction('Moved', async () => {
      const response = await moveFilePath(source, targetDirectory);
      setFileTabs((current) =>
        current.map((tab) => {
          const nextPath = replacePathPrefix(tab.path, source, response.path);
          return nextPath ? { ...tab, path: nextPath, preview: { ...tab.preview, path: nextPath } } : tab;
        }),
      );
      if (activeCenterTab.startsWith('file:')) {
        const activePath = activeCenterTab.slice('file:'.length);
        const nextActivePath = replacePathPrefix(activePath, source, response.path);
        if (nextActivePath) setActiveCenterTab(fileTabId(nextActivePath));
      }
      await refresh();
      return response.path;
    });
  };

  const handleDeletePath = async (path: string) => {
    await runAction('Deleted', async () => {
      await deleteFilePath(path);
      setFileTabs((current) => current.filter((tab) => tab.path !== path && !tab.path.startsWith(`${path}/`)));
      if (activeCenterTab === fileTabId(path) || activeCenterTab.startsWith(`file:${path}/`)) setActiveCenterTab('pipeline');
      await refresh();
      return path;
    });
  };

  const handleUploadFiles = async (targetDirectory: string, files: File[]) => {
    await runAction('Uploaded', async () => {
      const payloads = await Promise.all(
        files.map(async (file) => ({
          name: file.name,
          content_base64: await readFileAsBase64(file),
        })),
      );
      const response = await uploadFilePayloads(targetDirectory, payloads);
      await refresh();
      return response.paths.join(', ');
    });
  };

  const handleOpenFileTab = async (path: string) => {
    const existing = fileTabs.find((tab) => tab.path === path);
    if (existing) {
      setActiveCenterTab(fileTabId(path));
      return;
    }

    await runAction('', async () => {
      const preview = await getFilePreview(path);
      setFileTabs((current) => [
        ...current,
        {
          path,
          preview,
          draft: preview.type === 'text' ? preview.content ?? '' : '',
          dirty: false,
          saving: false,
        },
      ]);
      setActiveCenterTab(fileTabId(path));
      return '';
    });
  };

  const handleChangeFileTab = (path: string, draft: string) => {
    setFileTabs((current) => current.map((tab) => (tab.path === path ? { ...tab, draft, dirty: true } : tab)));
  };

  const handleCloseFileTab = (path: string) => {
    setFileTabs((current) => current.filter((tab) => tab.path !== path));
    if (activeCenterTab === fileTabId(path)) setActiveCenterTab('pipeline');
  };

  const handleSaveFileTab = async (path: string) => {
    const tab = fileTabs.find((item) => item.path === path);
    if (!tab || tab.preview.type !== 'text' || !tab.dirty || tab.saving) return;

    setFileTabs((current) => current.map((item) => (item.path === path ? { ...item, saving: true } : item)));
    try {
      const response = await updateFileContent(path, tab.draft);
      const preview = await getFilePreview(response.path);
      setFileTabs((current) =>
        current.map((item) =>
          item.path === path
            ? {
                ...item,
                path: response.path,
                preview,
                draft: preview.type === 'text' ? preview.content ?? '' : '',
                dirty: false,
                saving: false,
              }
            : item,
        ),
      );
      setActiveCenterTab(fileTabId(response.path));
      setNotice(`Saved: ${response.path}`);
    } catch (err) {
      setFileTabs((current) => current.map((item) => (item.path === path ? { ...item, saving: false } : item)));
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const runAction = async (label: string, action: () => Promise<string>) => {
    setError(null);
    setNotice(null);
    try {
      const subject = await action();
      if (label) setNotice(subject ? `${label}: ${subject}` : label);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div className="app-shell">
      {!snapshot?.project_path && (
        <ProjectStart
          directoryListing={directoryListing}
          directoryPickerOpen={directoryPickerOpen}
          directoryPickerMode={directoryPickerMode}
          onShowCreateProjectPicker={() => {
            handleShowCreateProjectPicker().catch((err: Error) => setError(err.message));
          }}
          onShowOpenProjectPicker={() => {
            handleShowOpenProjectPicker().catch((err: Error) => setError(err.message));
          }}
          onCloseDirectoryPicker={() => setDirectoryPickerOpen(false)}
          onBrowseDirectory={(path) => {
            handleBrowseDirectory(path).catch((err: Error) => setError(err.message));
          }}
          onCreateProjectPath={(parentPath, projectName) => {
            handleCreateProject(parentPath, projectName).catch((err: Error) => setError(err.message));
          }}
          onOpenProjectPath={(path) => {
            handleOpenProject(path).catch((err: Error) => setError(err.message));
          }}
        />
      )}
      <header className="topbar">
        <div className="brand">Open Pipeline Manager</div>
        <div className="status-strip">
          <span className={blockers ? 'metric blocker' : 'metric ok'}>
            {blockers ? <CircleStop size={15} /> : <CheckCircle2 size={15} />} {blockers} blockers
          </span>
          <span className="metric warning">
            <AlertTriangle size={15} /> {warnings} warnings
          </span>
          <button className="command-button" onClick={handleRunSelected} disabled={executionRunning}>
            {executionRunning ? <Loader2 className="spin" size={16} /> : <Play size={16} />}
            {executionRunning ? 'Running' : 'Run selected'}
          </button>
          <button
            className="command-button"
            onClick={() => {
              setEditingStep(null);
              setStepDialogOpen(true);
            }}
            disabled={!projectPath}
          >
            <Plus size={16} />
            Create step
          </button>
        </div>
      </header>

      {error && <div className="toast">{error}</div>}
      {!error && notice && <div className="toast notice-toast">{notice}</div>}

      <div className="workspace">
        <ProjectTree
          projectPath={projectPath}
          pipelinePath={pipelinePath}
          tree={tree}
          directoryListing={directoryListing}
          directoryPickerOpen={directoryPickerOpen}
          directoryPickerMode={directoryPickerMode}
          filePickerOpen={filePickerOpen}
          fileListing={fileListing}
          onShowCreateProjectPicker={() => {
            handleShowCreateProjectPicker().catch((err: Error) => setError(err.message));
          }}
          onShowOpenProjectPicker={() => {
            handleShowOpenProjectPicker().catch((err: Error) => setError(err.message));
          }}
          onCloseDirectoryPicker={() => setDirectoryPickerOpen(false)}
          onBrowseDirectory={(path) => {
            handleBrowseDirectory(path).catch((err: Error) => setError(err.message));
          }}
          onCreateProjectPath={(parentPath, projectName) => {
            handleCreateProject(parentPath, projectName).catch((err: Error) => setError(err.message));
          }}
          onOpenProjectPath={(path) => {
            handleOpenProject(path).catch((err: Error) => setError(err.message));
          }}
          onShowPipelinePicker={() => {
            handleShowPipelinePicker().catch((err: Error) => setError(err.message));
          }}
          onClosePipelinePicker={() => setFilePickerOpen(false)}
          onBrowseFiles={(path) => {
            handleBrowsePipelineFiles(path).catch((err: Error) => setError(err.message));
          }}
          onSelectPipelineFile={(path) => {
            handleSelectPipelineFile(path).catch((err: Error) => setError(err.message));
          }}
          onRefresh={() => refresh().catch((err: Error) => setError(err.message))}
          onCreatePath={(path, directory) => {
            handleCreatePath(path, directory).catch((err: Error) => setError(err.message));
          }}
          onRenamePath={(path, name) => {
            handleRenamePath(path, name).catch((err: Error) => setError(err.message));
          }}
          onMovePath={(source, targetDirectory) => {
            handleMovePath(source, targetDirectory).catch((err: Error) => setError(err.message));
          }}
          onDeletePath={(path) => {
            handleDeletePath(path).catch((err: Error) => setError(err.message));
          }}
          onUploadFiles={(targetDirectory, files) => {
            handleUploadFiles(targetDirectory, files).catch((err: Error) => setError(err.message));
          }}
          onOpenFileTab={(path) => {
            handleOpenFileTab(path).catch((err: Error) => setError(err.message));
          }}
        />
        <CenterTabs
          activeTab={activeCenterTab}
          fileTabs={fileTabs}
          onSelectTab={setActiveCenterTab}
          onCloseFileTab={handleCloseFileTab}
          onChangeFileTab={handleChangeFileTab}
          onSaveFileTab={(path) => {
            handleSaveFileTab(path).catch((err: Error) => setError(err.message));
          }}
          pipelineCanvas={
            <PipelineCanvas
              pipeline={snapshot?.pipeline}
              snapshot={snapshot}
              validation={validation}
              selectedStepId={selectedStepId}
              onSelectStep={(stepId) => {
                handleSelectStep(stepId).catch((err: Error) => setError(err.message));
              }}
              onToggleStep={(stepId, selected) => {
                handleToggleStep(stepId, selected).catch((err: Error) => setError(err.message));
              }}
              onRunStep={(stepId) => {
                handleRunStep(stepId).catch((err: Error) => setError(err.message));
              }}
              executionRunning={executionRunning}
              onSaveLayout={(positions) => {
                saveLayout(positions).catch((err: Error) => setError(err.message));
              }}
              onConnectSteps={(source, target) => {
                handleConnectSteps(source, target).catch((err: Error) => setError(err.message));
              }}
              onDisconnectSteps={(edges) => {
                handleDisconnectSteps(edges).catch((err: Error) => setError(err.message));
              }}
              onReconnectStep={(edge, connection) => {
                handleReconnectStep(edge, connection).catch((err: Error) => setError(err.message));
              }}
            />
          }
        />
        <StepPanel
          detail={detail ? { ...detail, validation: selectedIssues.length ? selectedIssues : detail.validation } : null}
          onChangeParams={(stepId, values) => {
            handleParams(stepId, values).catch((err: Error) => setError(err.message));
          }}
          onEditStep={(step) => {
            setEditingStep(step);
            setStepDialogOpen(true);
          }}
          onRunStep={(stepId) => {
            handleRunStep(stepId).catch((err: Error) => setError(err.message));
          }}
          executionRunning={executionRunning}
        />
      </div>

      {stepDialogOpen && (
        <CreateStepDialog
          suggestedIndex={snapshot?.pipeline?.steps.length ?? 0}
          mode={editingStep ? 'edit' : 'create'}
          initialStep={editingStep}
          pipelineSteps={snapshot?.pipeline?.steps ?? []}
          onCreate={(step) => {
            const action = editingStep ? handleEditStep : handleCreateStep;
            action(step).catch((err: Error) => setError(err.message));
          }}
          onClose={() => {
            setStepDialogOpen(false);
            setEditingStep(null);
          }}
        />
      )}

      <LogsPanel selectedStepId={selectedStepId} content={logs} onStop={handleStop} />
    </div>
  );
}

function ProjectStart({
  directoryListing,
  directoryPickerOpen,
  directoryPickerMode,
  onShowCreateProjectPicker,
  onShowOpenProjectPicker,
  onCloseDirectoryPicker,
  onBrowseDirectory,
  onCreateProjectPath,
  onOpenProjectPath,
}: {
  directoryListing?: DirectoryListing | null;
  directoryPickerOpen: boolean;
  directoryPickerMode: 'create' | 'open';
  onShowCreateProjectPicker: () => void;
  onShowOpenProjectPicker: () => void;
  onCloseDirectoryPicker: () => void;
  onBrowseDirectory: (path?: string) => void;
  onCreateProjectPath: (parentPath: string, projectName: string) => void;
  onOpenProjectPath: (path: string) => void;
}) {
  return (
    <div className="project-start">
      <div className="project-start-actions">
        <div className="brand">Open Pipeline Manager</div>
        <button className="command-button" onClick={onShowCreateProjectPicker}>
          Create Project
        </button>
        <button className="command-button" onClick={onShowOpenProjectPicker}>
          Open Project
        </button>
      </div>
      {directoryPickerOpen && (
        <DirectoryPicker
          mode={directoryPickerMode}
          listing={directoryListing}
          onBrowse={onBrowseDirectory}
          onCreate={onCreateProjectPath}
          onOpen={onOpenProjectPath}
          onClose={onCloseDirectoryPicker}
        />
      )}
    </div>
  );
}

function CenterTabs({
  activeTab,
  fileTabs,
  pipelineCanvas,
  onSelectTab,
  onCloseFileTab,
  onChangeFileTab,
  onSaveFileTab,
}: {
  activeTab: string;
  fileTabs: FileTab[];
  pipelineCanvas: ReactNode;
  onSelectTab: (tabId: string) => void;
  onCloseFileTab: (path: string) => void;
  onChangeFileTab: (path: string, draft: string) => void;
  onSaveFileTab: (path: string) => void;
}) {
  const activeFilePath = activeTab.startsWith('file:') ? activeTab.slice('file:'.length) : null;
  const activeFileTab = activeFilePath ? fileTabs.find((tab) => tab.path === activeFilePath) : null;

  return (
    <main className="center-panel">
      <div className="tab-strip">
        <button className={`tab-button ${activeTab === 'pipeline' ? 'is-active' : ''}`} onClick={() => onSelectTab('pipeline')}>
          Pipeline
        </button>
        {fileTabs.map((tab) => (
          <button
            key={tab.path}
            className={`tab-button file-tab-button ${activeTab === fileTabId(tab.path) ? 'is-active' : ''}`}
            onClick={() => onSelectTab(fileTabId(tab.path))}
            title={tab.path}
          >
            <span>{displayName(tab.path)}</span>
            {tab.dirty && <span className="dirty-dot" />}
            <span
              className="tab-close"
              role="button"
              tabIndex={0}
              onClick={(event) => {
                event.stopPropagation();
                onCloseFileTab(tab.path);
              }}
              onKeyDown={(event) => {
                if (event.key === 'Enter' || event.key === ' ') {
                  event.preventDefault();
                  event.stopPropagation();
                  onCloseFileTab(tab.path);
                }
              }}
              title="Close file"
            >
              <X size={13} />
            </span>
          </button>
        ))}
      </div>
      <div className="center-tab-content">
        {activeTab === 'pipeline' || !activeFileTab ? (
          pipelineCanvas
        ) : (
          <FileTabEditor tab={activeFileTab} onChange={onChangeFileTab} onSave={onSaveFileTab} />
        )}
      </div>
    </main>
  );
}

function FileTabEditor({
  tab,
  onChange,
  onSave,
}: {
  tab: FileTab;
  onChange: (path: string, draft: string) => void;
  onSave: (path: string) => void;
}) {
  if (tab.preview.type === 'image' && tab.preview.content) {
    return (
      <div className="center-file-viewer">
        <div className="center-file-toolbar">
          <span>{tab.path}</span>
          <code>{tab.preview.media_type ?? 'image'}</code>
        </div>
        <div className="image-viewer-surface">
          <img src={`data:${tab.preview.media_type};base64,${tab.preview.content}`} alt={tab.path} />
        </div>
      </div>
    );
  }

  if (tab.preview.type !== 'text') {
    return (
      <div className="center-file-viewer">
        <div className="center-file-toolbar">
          <span>{tab.path}</span>
        </div>
        <div className="empty">This file cannot be edited as text.</div>
      </div>
    );
  }

  return (
    <div className="center-file-editor-shell">
      <div className="center-file-toolbar">
        <span>{tab.path}</span>
        <button className="command-button" disabled={!tab.dirty || tab.saving} onClick={() => onSave(tab.path)}>
          {tab.saving ? <Loader2 className="spin" size={15} /> : <Save size={15} />}
          {tab.saving ? 'Saving' : 'Save'}
        </button>
      </div>
      <textarea
        className="center-file-editor"
        value={tab.draft}
        spellCheck={false}
        onChange={(event) => onChange(tab.path, event.target.value)}
      />
    </div>
  );
}

function readFileAsBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error(`Failed to read ${file.name}`));
    reader.onload = () => {
      const result = String(reader.result ?? '');
      resolve(result.includes(',') ? result.split(',')[1] : result);
    };
    reader.readAsDataURL(file);
  });
}

function joinFilesystemPath(parentPath: string, childName: string): string {
  const cleanName = sanitizeProjectName(childName);
  return parentPath.endsWith('/') ? `${parentPath}${cleanName}` : `${parentPath}/${cleanName}`;
}

function replacePathPrefix(path: string, oldPrefix: string, newPrefix: string): string | null {
  if (path === oldPrefix) return newPrefix;
  if (path.startsWith(`${oldPrefix}/`)) return `${newPrefix}${path.slice(oldPrefix.length)}`;
  return null;
}

function fileTabId(path: string): string {
  return `file:${path}`;
}

function sanitizeProjectName(name: string): string {
  return name
    .trim()
    .replace(/\s+/g, '_')
    .replace(/[^\w.-]/g, '_')
    .replace(/_+/g, '_')
    .replace(/^[._]+|[._]+$/g, '') || 'project';
}

function displayName(path?: string | null): string {
  if (!path) return '';
  const normalized = path.replace(/\\/g, '/').replace(/\/+$/g, '');
  return normalized.split('/').pop() || normalized || '';
}

const SESSION_KEY = 'open-pipeline-manager-session';
const SESSION_TTL_MS = 60 * 60 * 1000;

interface StoredSession {
  projectPath: string;
  pipelinePath?: string;
  expiresAt: number;
}

function loadStoredSession(): StoredSession | null {
  const raw = window.localStorage.getItem(SESSION_KEY);
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw) as StoredSession;
    if (!parsed.projectPath || parsed.expiresAt < Date.now()) {
      clearStoredSession();
      return null;
    }
    return parsed;
  } catch {
    clearStoredSession();
    return null;
  }
}

function saveStoredSession(projectPath: string, pipelinePath?: string): void {
  window.localStorage.setItem(
    SESSION_KEY,
    JSON.stringify({
      projectPath,
      pipelinePath,
      expiresAt: Date.now() + SESSION_TTL_MS,
    }),
  );
}

function clearStoredSession(): void {
  window.localStorage.removeItem(SESSION_KEY);
}
