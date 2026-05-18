import { useMemo, useRef, useState } from 'react';
import {
  ChevronDown,
  ChevronRight,
  File as FileIcon,
  FilePlus,
  FileText,
  Folder,
  FolderOpen,
  FolderPlus,
  Image,
  Pencil,
  RefreshCw,
  Trash2,
  Upload,
  X,
} from 'lucide-react';
import type { DirectoryListing, FileListing, TreeNode } from '../../types/pipeline';

interface ProjectTreeProps {
  projectPath: string;
  pipelinePath: string;
  tree?: TreeNode | null;
  directoryListing?: DirectoryListing | null;
  directoryPickerOpen: boolean;
  directoryPickerMode: 'create' | 'open';
  filePickerOpen: boolean;
  fileListing?: FileListing | null;
  onShowCreateProjectPicker: () => void;
  onShowOpenProjectPicker: () => void;
  onCloseDirectoryPicker: () => void;
  onBrowseDirectory: (path?: string) => void;
  onCreateProjectPath: (parentPath: string, projectName: string) => void;
  onOpenProjectPath: (path: string) => void;
  onShowPipelinePicker: () => void;
  onClosePipelinePicker: () => void;
  onBrowseFiles: (path?: string) => void;
  onSelectPipelineFile: (path: string) => void;
  onRefresh: () => void;
  onCreatePath: (path: string, directory: boolean) => void;
  onRenamePath: (path: string, name: string) => void;
  onMovePath: (source: string, targetDirectory: string) => void;
  onDeletePath: (path: string) => void;
  onUploadFiles: (targetDirectory: string, files: File[]) => void;
  onOpenFileTab: (path: string) => void;
}

export function ProjectTree({
  projectPath,
  pipelinePath,
  tree,
  directoryListing,
  directoryPickerOpen,
  directoryPickerMode,
  filePickerOpen,
  fileListing,
  onShowCreateProjectPicker,
  onShowOpenProjectPicker,
  onCloseDirectoryPicker,
  onBrowseDirectory,
  onCreateProjectPath,
  onOpenProjectPath,
  onShowPipelinePicker,
  onClosePipelinePicker,
  onBrowseFiles,
  onSelectPipelineFile,
  onRefresh,
  onCreatePath,
  onRenamePath,
  onMovePath,
  onDeletePath,
  onUploadFiles,
  onOpenFileTab,
}: ProjectTreeProps) {
  const [expanded, setExpanded] = useState<Set<string>>(new Set(['.', 'Input', 'steps', 'outputs']));
  const [selectedPath, setSelectedPath] = useState<string>('.');
  const uploadInputRef = useRef<HTMLInputElement | null>(null);
  const selectedNode = useMemo(() => (tree ? findNode(tree, selectedPath) : null), [tree, selectedPath]);
  const selectedDirectory = selectedNode?.type === 'directory' ? selectedNode.path : parentDirectory(selectedPath);

  const updateExpanded = (path: string, open: boolean) => {
    setExpanded((current) => {
      const next = new Set(current);
      if (open) next.add(path);
      else next.delete(path);
      return next;
    });
  };

  const createPath = (directory: boolean) => {
    const name = window.prompt(directory ? 'Folder name' : 'File name');
    if (!name) return;
    onCreatePath(joinPath(selectedDirectory, name), directory);
    updateExpanded(selectedDirectory, true);
  };

  const renamePath = () => {
    if (!selectedNode || selectedNode.path === '.') return;
    const name = window.prompt('New name', selectedNode.name);
    if (!name || name === selectedNode.name) return;
    onRenamePath(selectedNode.path, name);
  };

  const deletePath = () => {
    if (!selectedNode || selectedNode.path === '.') return;
    if (window.confirm(`Delete ${selectedNode.path}?`)) {
      onDeletePath(selectedNode.path);
      setSelectedPath('.');
    }
  };

  const uploadSelectedFiles = (files: FileList | null) => {
    const nextFiles = Array.from(files ?? []);
    if (nextFiles.length === 0) return;
    onUploadFiles(selectedDirectory, nextFiles);
    updateExpanded(selectedDirectory, true);
    if (uploadInputRef.current) uploadInputRef.current.value = '';
  };

  return (
    <aside className="project-panel">
      <div className="panel-title">
        <FolderOpen size={17} />
        <span>{displayName(projectPath) || 'Explorer'}</span>
      </div>

      <div className="stack">
        <div className="project-button-grid">
          <button className="command-button" onClick={onShowCreateProjectPicker} title="Choose a parent folder and create a named project">
            <FolderPlus size={16} />
            Create
          </button>
          <button className="command-button" onClick={onShowOpenProjectPicker} title="Browse and open an existing project folder">
            <FolderOpen size={16} />
            Open
          </button>
          <button className="icon-button" onClick={onRefresh} title="Refresh">
            <RefreshCw size={16} />
          </button>
        </div>
      </div>

      <div className="explorer-actions">
        <button className="icon-button" onClick={() => createPath(false)} title="New file">
          <FilePlus size={16} />
        </button>
        <button className="icon-button" onClick={() => createPath(true)} title="New folder">
          <FolderPlus size={16} />
        </button>
        <button className="icon-button" onClick={() => uploadInputRef.current?.click()} title="Upload files">
          <Upload size={16} />
        </button>
        <button className="icon-button" onClick={renamePath} disabled={!selectedNode || selectedNode.path === '.'} title="Rename">
          <Pencil size={16} />
        </button>
        <button className="icon-button danger-button" onClick={deletePath} disabled={!selectedNode || selectedNode.path === '.'} title="Delete">
          <Trash2 size={16} />
        </button>
        <input ref={uploadInputRef} className="hidden-file-input" type="file" multiple onChange={(event) => uploadSelectedFiles(event.target.files)} />
      </div>

      <div className="stack">
        {pipelinePath && <div className="selected-file-name">{displayName(pipelinePath)}</div>}
        <button className="command-button" onClick={onShowPipelinePicker}>
          <Upload size={16} />
          Import pipeline
        </button>
      </div>

      <div
        className="tree vscode-tree"
        onDragOver={(event) => event.preventDefault()}
        onDrop={(event) => {
          event.preventDefault();
          handleTreeDrop(event, '.', onMovePath, onUploadFiles);
          updateExpanded('.', true);
        }}
      >
        {tree ? (
          <TreeItem
            node={tree}
            depth={0}
            expanded={expanded}
            selectedPath={selectedPath}
            onSelect={(node) => {
              setSelectedPath(node.path);
              if (node.type === 'directory') updateExpanded(node.path, !expanded.has(node.path));
            }}
            onOpenFileTab={onOpenFileTab}
            onToggle={(path) => updateExpanded(path, !expanded.has(path))}
            onMovePath={onMovePath}
            onUploadFiles={onUploadFiles}
            onExpand={(path) => updateExpanded(path, true)}
          />
        ) : (
          <div className="empty">No files</div>
        )}
      </div>

      <div className="selected-path-panel">
        <span>Path</span>
        <code>{selectedPath === '.' ? displayName(projectPath) || '.' : selectedPath}</code>
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

      {filePickerOpen && (
        <PipelineFilePicker
          listing={fileListing}
          onBrowse={onBrowseFiles}
          onSelect={onSelectPipelineFile}
          onClose={onClosePipelinePicker}
        />
      )}
    </aside>
  );
}

function PipelineFilePicker({
  listing,
  onBrowse,
  onSelect,
  onClose,
}: {
  listing?: FileListing | null;
  onBrowse: (path?: string) => void;
  onSelect: (path: string) => void;
  onClose: () => void;
}) {
  return (
    <div className="modal-backdrop">
      <div className="directory-dialog">
        <div className="dialog-header">
          <span>Select Pipeline File</span>
          <button className="icon-button" onClick={onClose} title="Close">
            <X size={16} />
          </button>
        </div>
        <div className="picker-toolbar">
          <button className="command-button" onClick={() => onBrowse(undefined)}>
            Home
          </button>
          <button className="command-button" onClick={() => onBrowse('/')}>
            Root
          </button>
          <span>{displayName(listing?.path) || 'Files'}</span>
        </div>
        <div className="directory-list">
          {listing?.parent && (
            <button
              className="directory-row"
              onClick={() => {
                onBrowse(listing.parent ?? undefined);
              }}
            >
              <FolderOpen size={15} />
              ..
            </button>
          )}
          {listing?.directories.map((directory) => (
            <button
              key={directory.path}
              className="directory-row"
              onClick={() => {
                onBrowse(directory.path);
              }}
            >
              <Folder size={15} />
              <span>{directory.name}</span>
            </button>
          ))}
          {listing?.files.map((file) => (
            <button key={file.path} className="directory-row" onClick={() => onSelect(file.path)} onDoubleClick={() => onSelect(file.path)}>
              <FileText size={15} />
              <span>{file.name}</span>
            </button>
          ))}
          {!listing && <div className="empty">Loading files</div>}
          {listing && listing.directories.length === 0 && listing.files.length === 0 && <div className="empty">No pipeline files</div>}
        </div>
      </div>
    </div>
  );
}

export function DirectoryPicker({
  mode,
  listing,
  onBrowse,
  onCreate,
  onOpen,
  onClose,
}: {
  mode: 'create' | 'open';
  listing?: DirectoryListing | null;
  onBrowse: (path?: string) => void;
  onCreate: (parentPath: string, projectName: string) => void;
  onOpen: (path: string) => void;
  onClose: () => void;
}) {
  const [projectName, setProjectName] = useState('');
  const isCreateMode = mode === 'create';
  const cleanProjectName = sanitizeProjectName(projectName);

  return (
    <div className="modal-backdrop">
      <div className="directory-dialog">
        <div className="dialog-header">
          <span>{isCreateMode ? 'Create Project' : 'Open Project Folder'}</span>
          <button className="icon-button" onClick={onClose} title="Close">
            <X size={16} />
          </button>
        </div>
        <div className="picker-toolbar">
          <button className="command-button" onClick={() => onBrowse(undefined)}>
            Home
          </button>
          <button className="command-button" onClick={() => onBrowse('/')}>
            Root
          </button>
          <span>{displayName(listing?.path) || 'Folders'}</span>
        </div>
        {isCreateMode && (
          <label className="field compact-field">
            <span>Project name</span>
            <input
              value={projectName}
              onChange={(event) => setProjectName(event.target.value)}
              placeholder="my-pipeline-project"
              aria-label="Project name"
            />
            {projectName && <small className="clean-name">Folder: {cleanProjectName}</small>}
          </label>
        )}
        <div className="directory-list">
          {listing?.parent && (
            <button
              className="directory-row"
              onClick={() => {
                onBrowse(listing.parent ?? undefined);
              }}
            >
              <FolderOpen size={15} />
              ..
            </button>
          )}
          {listing?.directories.map((directory) => (
            <button
              key={directory.path}
              className="directory-row"
              onClick={() => {
                onBrowse(directory.path);
              }}
              onDoubleClick={() => {
                if (!isCreateMode) onOpen(directory.path);
              }}
            >
              <Folder size={15} />
              <span>{directory.name}</span>
            </button>
          ))}
          {!listing && <div className="empty">Loading folders</div>}
          {listing && listing.directories.length === 0 && <div className="empty">No visible folders</div>}
        </div>
        <div className="dialog-footer">
          <span>{isCreateMode ? `Parent: ${displayName(listing?.path) || 'Folder'}` : displayName(listing?.path)}</span>
          <button
            className="command-button"
            disabled={!listing?.path || (isCreateMode && !cleanProjectName)}
            onClick={() => {
              if (!listing?.path) return;
              if (isCreateMode) onCreate(listing.path, cleanProjectName);
              else onOpen(listing.path);
            }}
          >
            {isCreateMode ? 'Create project' : 'Open this folder'}
          </button>
        </div>
      </div>
    </div>
  );
}

function TreeItem({
  node,
  depth,
  expanded,
  selectedPath,
  onSelect,
  onOpenFileTab,
  onToggle,
  onMovePath,
  onUploadFiles,
  onExpand,
}: {
  node: TreeNode;
  depth: number;
  expanded: Set<string>;
  selectedPath: string;
  onSelect: (node: TreeNode) => void;
  onOpenFileTab: (path: string) => void;
  onToggle: (path: string) => void;
  onMovePath: (source: string, targetDirectory: string) => void;
  onUploadFiles: (targetDirectory: string, files: File[]) => void;
  onExpand: (path: string) => void;
}) {
  const isDirectory = node.type === 'directory';
  const isExpanded = expanded.has(node.path);
  const Icon = isDirectory ? (isExpanded ? FolderOpen : Folder) : iconForFile(node.name);
  const Disclosure = isDirectory ? (isExpanded ? ChevronDown : ChevronRight) : null;

  return (
    <div>
      <button
        className={`tree-item ${selectedPath === node.path ? 'is-active' : ''}`}
        style={{ paddingLeft: `${depth * 14 + 6}px` }}
        draggable={node.path !== '.'}
        onClick={() => onSelect(node)}
        onDoubleClick={(event) => {
          if (isDirectory) return;
          event.stopPropagation();
          onOpenFileTab(node.path);
        }}
        onDragStart={(event) => {
          event.dataTransfer.setData('application/x-opm-path', node.path);
          event.dataTransfer.effectAllowed = 'move';
        }}
        onDragOver={(event) => {
          if (isDirectory) event.preventDefault();
        }}
        onDrop={(event) => {
          if (!isDirectory) return;
          event.preventDefault();
          handleTreeDrop(event, node.path, onMovePath, onUploadFiles);
          onExpand(node.path);
        }}
        title={node.path}
      >
        <span className="disclosure">{Disclosure ? <Disclosure size={13} /> : null}</span>
        <Icon size={14} />
        <span>{node.name}</span>
      </button>
      {isDirectory && isExpanded
        ? node.children.map((child) => (
            <TreeItem
              key={child.path}
              node={child}
              depth={depth + 1}
              expanded={expanded}
              selectedPath={selectedPath}
              onSelect={onSelect}
              onOpenFileTab={onOpenFileTab}
              onToggle={onToggle}
              onMovePath={onMovePath}
              onUploadFiles={onUploadFiles}
              onExpand={onExpand}
            />
          ))
        : null}
    </div>
  );
}

function handleTreeDrop(
  event: React.DragEvent,
  targetDirectory: string,
  onMovePath: (source: string, targetDirectory: string) => void,
  onUploadFiles: (targetDirectory: string, files: File[]) => void,
) {
  const movedPath = event.dataTransfer.getData('application/x-opm-path');
  if (movedPath) {
    onMovePath(movedPath, targetDirectory);
    return;
  }

  const files = Array.from(event.dataTransfer.files ?? []);
  if (files.length > 0) {
    onUploadFiles(targetDirectory, files);
  }
}

function findNode(node: TreeNode, path: string): TreeNode | null {
  if (node.path === path) return node;
  for (const child of node.children) {
    const match = findNode(child, path);
    if (match) return match;
  }
  return null;
}

function parentDirectory(path: string): string {
  if (!path || path === '.') return '.';
  const parts = path.split('/');
  parts.pop();
  return parts.length ? parts.join('/') : '.';
}

function joinPath(directory: string, name: string): string {
  return directory === '.' ? name : `${directory}/${name}`;
}

function displayName(path?: string | null): string {
  if (!path) return '';
  const normalized = path.replace(/\\/g, '/').replace(/\/+$/g, '');
  if (normalized === '/') return 'Root';
  return normalized.split('/').pop() || normalized;
}

function sanitizeProjectName(name: string): string {
  return (
    name
      .trim()
      .replace(/\s+/g, '_')
      .replace(/[^\w.-]/g, '_')
      .replace(/_+/g, '_')
      .replace(/^[._]+|[._]+$/g, '') || 'project'
  );
}

function iconForFile(name: string) {
  const suffix = name.split('.').pop()?.toLowerCase();
  if (suffix && ['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg'].includes(suffix)) return Image;
  if (
    suffix &&
    [
      'txt',
      'md',
      'json',
      'yml',
      'yaml',
      'py',
      'ts',
      'tsx',
      'js',
      'css',
      'html',
      'log',
      'csv',
      'tsv',
      'fasta',
      'fa',
      'fna',
      'faa',
      'ffn',
      'fastq',
      'fq',
      'nwk',
      'newick',
      'nex',
      'phy',
    ].includes(suffix)
  )
    return FileText;
  return FileIcon;
}
