import { useCallback, useEffect } from 'react';
import {
  Background,
  BaseEdge,
  Controls,
  Handle,
  MarkerType,
  MiniMap,
  Position,
  ReactFlow,
  type Connection,
  type Edge,
  type EdgeProps,
  type Node,
  type NodeProps,
  useEdgesState,
  useNodesState,
} from '@xyflow/react';
import { Check, CircleAlert, CircleStop, Loader2, Play, Square } from 'lucide-react';
import type { PipelineConfig, PipelineStep, ProjectSnapshot, StepRuntimeState, ValidationIssue } from '../../types/pipeline';

interface PipelineCanvasProps {
  pipeline?: PipelineConfig | null;
  snapshot?: ProjectSnapshot | null;
  validation: ValidationIssue[];
  selectedStepId?: string | null;
  executionRunning: boolean;
  onSelectStep: (stepId: string) => void;
  onToggleStep: (stepId: string, selected: boolean) => void;
  onRunStep: (stepId: string) => void;
  onSaveLayout: (positions: Record<string, { x: number; y: number }>) => void;
  onConnectSteps: (source: string, target: string) => void;
  onDisconnectSteps: (edges: Array<{ source: string; target: string }>) => void;
  onReconnectStep: (edge: { source: string; target: string }, connection: Connection) => void;
}

interface StepNodeData extends Record<string, unknown> {
  step: PipelineStep;
  state?: StepRuntimeState;
  issues: ValidationIssue[];
  selectedStepId?: string | null;
  executionRunning: boolean;
  onSelectStep: (stepId: string) => void;
  onToggleStep: (stepId: string, selected: boolean) => void;
  onRunStep: (stepId: string) => void;
}

type PipelineNode = Node<StepNodeData>;
type PipelineEdgeData = {
  lane: 'order' | 'output';
  skip: number;
};
type PipelineEdge = Edge<PipelineEdgeData>;

const nodeTypes = { stepNode: StepNode };
const edgeTypes = { routed: RoutedEdge };
const NODE_START_X = 80;
const NODE_START_Y = 120;
const NODE_GAP_X = 330;

export function PipelineCanvas({
  pipeline,
  snapshot,
  validation,
  selectedStepId,
  executionRunning,
  onSelectStep,
  onToggleStep,
  onRunStep,
  onSaveLayout,
  onConnectSteps,
  onDisconnectSteps,
  onReconnectStep,
}: PipelineCanvasProps) {
  const [nodes, setNodes, onNodesChange] = useNodesState<PipelineNode>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<PipelineEdge>([]);

  useEffect(() => {
    if (!pipeline) {
      setNodes([]);
      setEdges([]);
      return;
    }

    const nextNodes: PipelineNode[] = pipeline.steps.map((step, index) => ({
      id: step.id,
      type: 'stepNode',
      position: snapshot?.visual_layout?.[step.id] ?? defaultStepPosition(index),
      data: {
        step,
        state: snapshot?.state?.[step.id],
        issues: validation.filter((issue) => issue.step_id === step.id),
        selectedStepId,
        executionRunning,
        onSelectStep,
        onToggleStep,
        onRunStep,
      },
    }));
    const nextEdges = buildStepEdges(pipeline, snapshot);
    setNodes(nextNodes);
    setEdges(nextEdges);
  }, [pipeline, snapshot, validation, selectedStepId, executionRunning, onSelectStep, onToggleStep, onRunStep, setNodes, setEdges]);

  const handleNodeDragStop = useCallback(
    (_event: unknown, node: PipelineNode) => {
      onSaveLayout({ [node.id]: node.position });
    },
    [onSaveLayout],
  );

  const handleConnect = useCallback(
    (connection: Connection) => {
      if (!connection.source || !connection.target || connection.source === connection.target) return;
      onConnectSteps(connection.source, connection.target);
    },
    [onConnectSteps],
  );

  const handleEdgesDelete = useCallback(
    (deletedEdges: Edge[]) => {
      onDisconnectSteps(deletedEdges.map((edge) => ({ source: edge.source, target: edge.target })));
    },
    [onDisconnectSteps],
  );

  const handleReconnect = useCallback(
    (edge: Edge, connection: Connection) => {
      onReconnectStep({ source: edge.source, target: edge.target }, connection);
    },
    [onReconnectStep],
  );

  return (
    <main className="canvas-panel">
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        edgeTypes={edgeTypes}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        onNodeDragStop={handleNodeDragStop}
        onConnect={handleConnect}
        onEdgesDelete={handleEdgesDelete}
        onReconnect={handleReconnect}
        isValidConnection={(connection) => Boolean(connection.source && connection.target && connection.source !== connection.target)}
        nodesConnectable
        edgesReconnectable
        edgesFocusable
        deleteKeyCode={['Backspace', 'Delete']}
        fitView
      >
        <Background color="#d7dee7" gap={22} />
        <Controls />
        <MiniMap pannable zoomable nodeStrokeWidth={3} />
      </ReactFlow>
    </main>
  );
}

function defaultStepPosition(index: number) {
  return { x: NODE_START_X + index * NODE_GAP_X, y: NODE_START_Y };
}

function buildStepEdges(pipeline: PipelineConfig, snapshot?: ProjectSnapshot | null): PipelineEdge[] {
  const stepIds = new Set(pipeline.steps.map((step) => step.id));
  const stepIndex = new Map(pipeline.steps.map((step, index) => [step.id, index]));
  const edges = new Map<string, PipelineEdge>();

  const addOrderEdge = (source: string, target: string) => {
    if (!stepIds.has(source) || !stepIds.has(target)) return;

    const edgeKey = `order:${source}->${target}`;
    if (edges.has(edgeKey)) return;

    const running = snapshot?.state?.[target]?.status === 'running';
    edges.set(edgeKey, {
      id: edgeKey,
      source,
      target,
      type: 'straight',
      animated: running,
      deletable: false,
      reconnectable: false,
      interactionWidth: 18,
      markerEnd: {
        type: MarkerType.ArrowClosed,
        color: running ? '#f59e0b' : '#475569',
      },
      style: {
        stroke: running ? '#f59e0b' : '#475569',
        strokeWidth: running ? 2.8 : 2.2,
      },
    });
  };

  const addOutputEdge = (source: string, target: string, sourceOutput: string, inputKey: string) => {
    if (!stepIds.has(source) || !stepIds.has(target)) return;

    const edgeKey = `output:${source}:${sourceOutput}->${target}:${inputKey}`;
    if (edges.has(edgeKey)) return;

    const running = snapshot?.state?.[target]?.status === 'running';
    const skip = Math.max(1, Math.abs((stepIndex.get(target) ?? 0) - (stepIndex.get(source) ?? 0)));
    edges.set(edgeKey, {
      id: edgeKey,
      source,
      target,
      type: 'routed',
      animated: running,
      deletable: false,
      reconnectable: false,
      data: {
        lane: 'output',
        skip,
      },
      interactionWidth: 14,
      markerEnd: {
        type: MarkerType.ArrowClosed,
        color: running ? '#f59e0b' : '#0f766e',
      },
      style: {
        stroke: running ? '#f59e0b' : '#0f766e',
        strokeDasharray: '7 6',
        strokeWidth: running ? 2.4 : 2,
      },
    });
  };

  pipeline.steps.slice(0, -1).forEach((step, index) => {
    addOrderEdge(step.id, pipeline.steps[index + 1].id);
  });

  pipeline.steps.forEach((step) => {
    step.inputs.forEach((input) => {
      if (input.source_step && input.source_output) {
        addOutputEdge(input.source_step, step.id, input.source_output, input.key);
      }
    });
  });

  return [...edges.values()];
}

function RoutedEdge({
  id,
  sourceX,
  sourceY,
  targetX,
  targetY,
  markerEnd,
  style,
  selected,
  data,
}: EdgeProps<PipelineEdge>) {
  const path = curvedPath(sourceX, sourceY, targetX, targetY, data?.lane ?? 'order', data?.skip ?? 1);
  return (
    <BaseEdge
      id={id}
      path={path}
      markerEnd={markerEnd}
      style={{
        ...style,
        strokeWidth: selected ? 3 : style?.strokeWidth,
      }}
      interactionWidth={18}
    />
  );
}

function curvedPath(
  sourceX: number,
  sourceY: number,
  targetX: number,
  targetY: number,
  lane: 'order' | 'output',
  skip: number,
) {
  const dx = targetX - sourceX;
  const direction = dx >= 0 ? 1 : -1;
  const distance = Math.max(Math.abs(dx), 1);
  const horizontalBend = Math.max(90, Math.min(distance * 0.42, 260));
  const verticalBend = lane === 'output' ? Math.min(360, 120 + skip * 64) : Math.min(150, 70 + skip * 18);
  const control1X = sourceX + direction * horizontalBend;
  const control2X = targetX - direction * horizontalBend;
  const control1Y = sourceY - verticalBend;
  const control2Y = targetY - verticalBend;

  return `M ${sourceX},${sourceY} C ${control1X},${control1Y} ${control2X},${control2Y} ${targetX},${targetY}`;
}

function StepNode(props: NodeProps<PipelineNode>) {
  const data = props.data;
  const { step, state, issues, selectedStepId, executionRunning, onSelectStep, onToggleStep, onRunStep } = data;
  const status = state?.status ?? 'pending';
  const blockers = issues.filter((issue) => issue.severity === 'blocker').length;
  const warnings = issues.filter((issue) => issue.severity === 'warning').length;
  const isSelected = selectedStepId === step.id;

  return (
    <div className="pipeline-node-shell">
      <Handle type="target" position={Position.Left} className="node-handle node-handle-target" />
      <div
        className={`pipeline-node status-${status} ${isSelected ? 'is-selected' : ''}`}
        onClick={() => onSelectStep(step.id)}
        onKeyDown={(event) => {
          if (event.key === 'Enter' || event.key === ' ') {
            event.preventDefault();
            onSelectStep(step.id);
          }
        }}
        role="button"
        tabIndex={0}
      >
        <div className="node-header">
          <input
            type="checkbox"
            checked={Boolean(state?.selected)}
            onChange={(event) => onToggleStep(step.id, event.target.checked)}
            onClick={(event) => event.stopPropagation()}
            aria-label={`Select ${step.id}`}
          />
          <span className="node-id">{step.id}</span>
          <StatusIcon status={status} />
        </div>
        <div className="node-name">{step.name}</div>
        <div className="node-description">{step.description || step.command || 'No command'}</div>
        <div className="node-footer">
          <span>{status}</span>
          <span className="issue-count">
            {blockers > 0 && (
              <>
                <CircleStop size={13} /> {blockers}
              </>
            )}
            {warnings > 0 && (
              <>
                <CircleAlert size={13} /> {warnings}
              </>
            )}
          </span>
          <button
            className="node-run"
            onClick={(event) => {
              event.stopPropagation();
              onRunStep(step.id);
            }}
            disabled={executionRunning}
            title="Run step"
          >
            {executionRunning ? <Loader2 className="spin" size={14} /> : <Play size={14} />}
          </button>
        </div>
      </div>
      <Handle type="source" position={Position.Right} className="node-handle node-handle-source" />
    </div>
  );
}

function StatusIcon({ status }: { status: string }) {
  if (status === 'running') return <Loader2 className="spin" size={16} />;
  if (status === 'ok') return <Check size={16} />;
  if (status === 'error') return <CircleStop size={16} />;
  return <Square size={14} />;
}
