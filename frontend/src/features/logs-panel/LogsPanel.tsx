import { Square } from 'lucide-react';

interface LogsPanelProps {
  selectedStepId?: string | null;
  content: string;
  onStop: () => void;
}

export function LogsPanel({ selectedStepId, content, onStop }: LogsPanelProps) {
  return (
    <section className="logs-panel">
      <div className="logs-header">
        <span>{selectedStepId ? `Logs: ${selectedStepId}` : 'Logs'}</span>
        <button className="icon-button" onClick={onStop} title="Stop execution">
          <Square size={15} />
        </button>
      </div>
      <pre>{content || 'No log output'}</pre>
    </section>
  );
}
