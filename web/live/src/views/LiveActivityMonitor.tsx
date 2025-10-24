import { Component, createSignal, For, onMount, onCleanup, Show, createMemo } from 'solid-js';
import { TrackedLiveAction } from '../client/model';
import { WebSocketService } from '../services/websocketService';
import { ActivityDetailPanel } from '../components/ActivityDetailPanel';
import './LiveActivityMonitor.css';

type ActionType = 'all' | 'upload' | 'download' | 'rbe' | 'clients' | 'other';
type ActionStatus = 'all' | 'completed' | 'in_progress' | 'pending' | 'failed' | 'canceled';

const wsService = new WebSocketService();

export const LiveActivityMonitor: Component = () => {
  const [actions, setActions] = createSignal<TrackedLiveAction[]>([]);
  const [selectedAction, setSelectedAction] = createSignal<TrackedLiveAction | null>(null);
  const [isRecording, setIsRecording] = createSignal(true);
  const [filterType, setFilterType] = createSignal<ActionType>('all');
  const [filterStatus, setFilterStatus] = createSignal<ActionStatus>('all');
  const [searchQuery, setSearchQuery] = createSignal('');

  onMount(() => {
    wsService.connect();
    const unsubscribe = wsService.subscribe(setActions);

    onCleanup(() => {
      unsubscribe();
      wsService.disconnect();
    });
  });

  const filteredActions = createMemo(() => {
    let filtered = actions();

    // Filter by type
    if (filterType() !== 'all') {
      filtered = filtered.filter((a) => a.type === filterType());
    }

    // Filter by status
    if (filterStatus() !== 'all') {
      filtered = filtered.filter((a) => a.status === filterStatus());
    }

    // Filter by search query
    const query = searchQuery().toLowerCase();
    if (query) {
      filtered = filtered.filter((a) => {
        const command = a.meta['rbe.command'] || '';
        const from = a.meta['general.from'] || '';
        return (
          a.uuid.toLowerCase().includes(query) ||
          command.toLowerCase().includes(query) ||
          from.toLowerCase().includes(query)
        );
      });
    }

    return filtered;
  });

  const getCountByType = (type: ActionType) => {
    if (type === 'all') return actions().length;
    return actions().filter((a) => a.type === type).length;
  };

  const getCountByStatus = (status: ActionStatus) => {
    if (status === 'all') return actions().length;
    return actions().filter((a) => a.status === status).length;
  };

  const toggleRecording = () => {
    const newState = !isRecording();
    setIsRecording(newState);
    // WebSocket connection is always on, this just controls UI updates
    if (!newState) {
      console.log('⏸️  Paused live updates (still receiving data)');
    } else {
      console.log('▶️  Resumed live updates');
    }
  };

  const formatTimestamp = (timestamp: number) => {
    const now = Date.now();
    const diff = now - timestamp;
    const seconds = Math.floor(diff / 1000);
    const minutes = Math.floor(seconds / 60);
    const hours = Math.floor(minutes / 60);

    if (seconds < 60) return `${seconds}s ago`;
    if (minutes < 60) return `${minutes}m ago`;
    return `${hours}h ago`;
  };

  return (
    <div class="live-activity-monitor">
      <div class="controls-bar">
        <div class="recording-controls">
          <button
            class="record-button"
            classList={{ recording: isRecording() }}
            onClick={toggleRecording}
            title={isRecording() ? 'Pause updates' : 'Resume updates'}
          >
            <span class="material-symbols-outlined">
              {isRecording() ? 'pause_circle' : 'play_circle'}
            </span>
          </button>
          <button class="clear-button" onClick={() => setActions([])} title="Clear all">
            <span class="material-symbols-outlined">delete</span>
          </button>
        </div>

        <div class="filter-chips">
          <div class="chip-group">
            <FilterChip
              label="All"
              count={getCountByType('all')}
              active={filterType() === 'all'}
              onClick={() => setFilterType('all')}
            />
            <FilterChip
              label="Upload"
              count={getCountByType('upload')}
              active={filterType() === 'upload'}
              onClick={() => setFilterType('upload')}
            />
            <FilterChip
              label="Download"
              count={getCountByType('download')}
              active={filterType() === 'download'}
              onClick={() => setFilterType('download')}
            />
            <FilterChip
              label="RBE"
              count={getCountByType('rbe')}
              active={filterType() === 'rbe'}
              onClick={() => setFilterType('rbe')}
            />
            <FilterChip
              label="Others"
              count={getCountByType('other')}
              active={filterType() === 'other'}
              onClick={() => setFilterType('other')}
            />
          </div>

          <div class="divider"></div>

          <div class="chip-group">
            <FilterChip
              label="All"
              count={getCountByStatus('all')}
              active={filterStatus() === 'all'}
              onClick={() => setFilterStatus('all')}
            />
            <FilterChip
              label="Completed"
              count={getCountByStatus('completed')}
              active={filterStatus() === 'completed'}
              onClick={() => setFilterStatus('completed')}
              color="success"
            />
            <FilterChip
              label="In Progress"
              count={getCountByStatus('in_progress')}
              active={filterStatus() === 'in_progress'}
              onClick={() => setFilterStatus('in_progress')}
              color="info"
            />
            <FilterChip
              label="Queued"
              count={getCountByStatus('pending')}
              active={filterStatus() === 'pending'}
              onClick={() => setFilterStatus('pending')}
              color="warning"
            />
            <FilterChip
              label="Failed"
              count={getCountByStatus('failed')}
              active={filterStatus() === 'failed'}
              onClick={() => setFilterStatus('failed')}
              color="error"
            />
            <FilterChip
              label="Canceled"
              count={getCountByStatus('canceled')}
              active={filterStatus() === 'canceled'}
              onClick={() => setFilterStatus('canceled')}
            />
          </div>
        </div>

        <div class="search-box">
          <span class="material-symbols-outlined">search</span>
          <input
            type="text"
            placeholder="Search activities..."
            value={searchQuery()}
            onInput={(e) => setSearchQuery(e.currentTarget.value)}
          />
        </div>
      </div>

      <div class="activity-list">
        <For each={filteredActions()}>
          {(action) => (
            <ActivityRow
              action={action}
              selected={selectedAction()?.uuid === action.uuid}
              onClick={() => setSelectedAction(action)}
              formatTimestamp={formatTimestamp}
            />
          )}
        </For>
        <Show when={filteredActions().length === 0}>
          <div class="empty-state">
            <span class="material-symbols-outlined">inbox</span>
            <p>No activities found</p>
          </div>
        </Show>
      </div>

      <Show when={selectedAction()}>
        <ActivityDetailPanel
          action={selectedAction()!}
          onClose={() => setSelectedAction(null)}
        />
      </Show>
    </div>
  );
};

const FilterChip: Component<{
  label: string;
  count: number;
  active: boolean;
  onClick: () => void;
  color?: 'success' | 'error' | 'warning' | 'info';
}> = (props) => {
  return (
    <button
      class="filter-chip"
      classList={{
        active: props.active,
        [props.color || '']: !!props.color,
      }}
      onClick={props.onClick}
    >
      <span class="label">{props.label}</span>
      <span class="count">{props.count}</span>
    </button>
  );
};

const ActivityRow: Component<{
  action: TrackedLiveAction;
  selected: boolean;
  onClick: () => void;
  formatTimestamp: (ts: number) => string;
}> = (props) => {
  const getStatusIcon = () => {
    switch (props.action.status) {
      case 'completed':
        return 'check_circle';
      case 'failed':
        return 'error';
      case 'in_progress':
        return 'pending';
      case 'pending':
        return 'schedule';
      case 'canceled':
        return 'cancel';
      default:
        return 'help';
    }
  };

  const getTypeLabel = () => {
    return props.action.type.toUpperCase();
  };

  const getActionLabel = () => {
    const cmd = props.action.meta['rbe.command'];
    if (cmd) {
      // Truncate long commands
      return cmd.length > 60 ? cmd.substring(0, 60) + '...' : cmd;
    }
    return `${props.action.type} operation`;
  };

  return (
    <div
      class="activity-row"
      classList={{
        selected: props.selected,
        [props.action.status]: true,
      }}
      onClick={props.onClick}
    >
      <div class="status-indicator">
        <span class="material-symbols-outlined">{getStatusIcon()}</span>
      </div>
      <div class="activity-content">
        <div class="activity-header">
          <span class="type-badge">{getTypeLabel()}</span>
          <span class="status-text">{props.action.status.replace('_', ' ')}</span>
          <span class="timestamp">{props.formatTimestamp(props.action.updated_at)}</span>
        </div>
        <div class="activity-details">
          <span class="action-label">{getActionLabel()}</span>
        </div>
        <div class="activity-meta">
          <span class="meta-item">
            <span class="material-symbols-outlined">computer</span>
            {props.action.meta['general.from'] || 'Unknown'}
          </span>
          <span class="meta-item">
            <span class="material-symbols-outlined">fingerprint</span>
            {props.action.uuid.substring(0, 8)}
          </span>
        </div>
      </div>
    </div>
  );
};
