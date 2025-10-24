import { Component, createSignal, For, onMount, onCleanup, Show, createMemo } from 'solid-js';
import { BuildInvocation } from '../client/model';
import { WebSocketService } from '../services/websocketService';
import './BuildInfoViewer.css';

const wsService = new WebSocketService();

export const BuildInfoViewer: Component = () => {
  const [builds, setBuilds] = createSignal<BuildInvocation[]>([]);
  const [selectedBuilds, setSelectedBuilds] = createSignal<Set<string>>(new Set());
  const [filterStatus, setFilterStatus] = createSignal<'all' | 'running' | 'succeeded' | 'failed'>('all');

  onMount(() => {
    wsService.connect();
    const unsubscribe = wsService.subscribeBuilds(setBuilds);

    // Also fetch from API for historical data
    fetchBuildsFromAPI();

    onCleanup(() => {
      unsubscribe();
      wsService.disconnect();
    });
  });

  const fetchBuildsFromAPI = async () => {
    try {
      const response = await fetch('http://localhost:3002/api/builds?limit=50');
      const data = await response.json();
      if (data.builds) {
        setBuilds(data.builds);
      }
    } catch (error) {
      console.error('Failed to fetch builds from API:', error);
    }
  };

  const filteredBuilds = createMemo(() => {
    const status = filterStatus();
    if (status === 'all') {
      return builds();
    }
    return builds().filter((b) => b.status === status);
  });

  const toggleBuildSelection = (invocationId: string) => {
    const newSelection = new Set(selectedBuilds());
    if (newSelection.has(invocationId)) {
      newSelection.delete(invocationId);
    } else {
      newSelection.add(invocationId);
    }
    setSelectedBuilds(newSelection);
  };

  const getTimeAgo = (timestamp: number) => {
    const seconds = Math.floor((Date.now() - timestamp) / 1000);
    if (seconds < 60) return `${seconds}s ago`;
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
    if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
    return `${Math.floor(seconds / 86400)}d ago`;
  };

  const getDuration = (build: BuildInvocation) => {
    if (!build.completed_at) return 'Running...';
    const durationMs = build.completed_at - build.started_at;
    const seconds = Math.floor(durationMs / 1000);
    if (seconds < 60) return `${seconds}s`;
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
    const hours = Math.floor(minutes / 60);
    return `${hours}h ${minutes % 60}m`;
  };

  const getStatusBadgeClass = (status: string) => {
    switch (status) {
      case 'running':
        return 'status-badge status-running';
      case 'succeeded':
        return 'status-badge status-succeeded';
      case 'failed':
        return 'status-badge status-failed';
      default:
        return 'status-badge';
    }
  };

  const truncateId = (id: string) => {
    return id.length > 12 ? `${id.substring(0, 12)}...` : id;
  };

  const selectedBuildsList = createMemo(() => {
    const selected = selectedBuilds();
    return builds().filter((b) => selected.has(b.invocation_id));
  });

  const buildStats = () => {
    return {
      total: builds().length,
      running: builds().filter((b) => b.status === 'running').length,
      succeeded: builds().filter((b) => b.status === 'succeeded').length,
      failed: builds().filter((b) => b.status === 'failed').length,
    };
  };

  return (
    <div class="build-info-viewer">
      <div class="viewer-header">
        <h2>
          <span class="material-symbols-outlined">construction</span>
          Build Invocations
        </h2>
        <div class="build-summary">
          <div class="summary-item">
            <span class="summary-label">Total:</span>
            <span class="summary-value">{buildStats().total}</span>
          </div>
          <div class="summary-item">
            <span class="summary-label">Running:</span>
            <span class="summary-value status-running">{buildStats().running}</span>
          </div>
          <div class="summary-item">
            <span class="summary-label">Succeeded:</span>
            <span class="summary-value status-succeeded">{buildStats().succeeded}</span>
          </div>
          <div class="summary-item">
            <span class="summary-label">Failed:</span>
            <span class="summary-value status-failed">{buildStats().failed}</span>
          </div>
        </div>
      </div>

      <div class="filter-bar">
        <div class="filter-chips">
          <button
            class="filter-chip"
            classList={{ active: filterStatus() === 'all' }}
            onClick={() => setFilterStatus('all')}
          >
            All ({builds().length})
          </button>
          <button
            class="filter-chip"
            classList={{ active: filterStatus() === 'running' }}
            onClick={() => setFilterStatus('running')}
          >
            Running ({buildStats().running})
          </button>
          <button
            class="filter-chip"
            classList={{ active: filterStatus() === 'succeeded' }}
            onClick={() => setFilterStatus('succeeded')}
          >
            Succeeded ({buildStats().succeeded})
          </button>
          <button
            class="filter-chip"
            classList={{ active: filterStatus() === 'failed' }}
            onClick={() => setFilterStatus('failed')}
          >
            Failed ({buildStats().failed})
          </button>
        </div>
      </div>

      <Show when={selectedBuilds().size >= 2}>
        <div class="comparison-view">
          <div class="comparison-header">
            <h3>Build Comparison ({selectedBuilds().size} selected)</h3>
            <button class="clear-selection-btn" onClick={() => setSelectedBuilds(new Set())}>
              Clear Selection
            </button>
          </div>
          <div class="comparison-cards">
            <For each={selectedBuildsList()}>
              {(build) => <BuildComparisonCard build={build} getDuration={getDuration} />}
            </For>
          </div>
        </div>
      </Show>

      <Show
        when={filteredBuilds().length > 0}
        fallback={
          <div class="empty-state">
            <span class="material-symbols-outlined">info</span>
            <p>No builds detected yet. Builds will appear here once Bazel invocations are tracked.</p>
          </div>
        }
      >
        <div class="builds-table">
          <table>
            <thead>
              <tr>
                <th>
                  <Show when={filteredBuilds().length > 0}>
                    <input
                      type="checkbox"
                      disabled
                      title="Select builds for comparison"
                    />
                  </Show>
                </th>
                <th>Invocation ID</th>
                <th>Started</th>
                <th>Tool</th>
                <th>Status</th>
                <th>Targets</th>
                <th>Actions</th>
                <th>Duration</th>
              </tr>
            </thead>
            <tbody>
              <For each={filteredBuilds()}>
                {(build) => (
                  <tr classList={{ selected: selectedBuilds().has(build.invocation_id) }}>
                    <td>
                      <input
                        type="checkbox"
                        checked={selectedBuilds().has(build.invocation_id)}
                        onChange={() => toggleBuildSelection(build.invocation_id)}
                      />
                    </td>
                    <td>
                      <code class="invocation-id" title={build.invocation_id}>
                        {truncateId(build.invocation_id)}
                      </code>
                    </td>
                    <td>
                      <span class="timestamp" title={new Date(build.started_at).toLocaleString()}>
                        {getTimeAgo(build.started_at)}
                      </span>
                    </td>
                    <td>
                      <span class="tool-info">
                        {build.tool_name} {build.tool_version}
                      </span>
                    </td>
                    <td>
                      <span class={getStatusBadgeClass(build.status)}>
                        {build.status}
                      </span>
                    </td>
                    <td>
                      <span class="count-badge">{build.targets.length}</span>
                    </td>
                    <td>
                      <span class="count-badge">{build.actions}</span>
                    </td>
                    <td>
                      <span class="duration">{getDuration(build)}</span>
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </div>
      </Show>
    </div>
  );
};

const BuildComparisonCard: Component<{
  build: BuildInvocation;
  getDuration: (build: BuildInvocation) => string;
}> = (props) => {
  const getStatusIcon = () => {
    switch (props.build.status) {
      case 'running':
        return 'pending';
      case 'succeeded':
        return 'check_circle';
      case 'failed':
        return 'error';
      default:
        return 'help';
    }
  };

  const getStatusClass = () => {
    switch (props.build.status) {
      case 'running':
        return 'comparison-card-running';
      case 'succeeded':
        return 'comparison-card-succeeded';
      case 'failed':
        return 'comparison-card-failed';
      default:
        return '';
    }
  };

  return (
    <div class={`comparison-card ${getStatusClass()}`}>
      <div class="comparison-card-header">
        <span class="material-symbols-outlined status-icon">{getStatusIcon()}</span>
        <div class="card-title">
          <h4>{props.build.tool_name}</h4>
          <span class="version">{props.build.tool_version}</span>
        </div>
      </div>
      <div class="comparison-card-body">
        <div class="metric-row">
          <span class="metric-label">Invocation ID:</span>
          <code class="metric-value">{props.build.invocation_id.substring(0, 16)}...</code>
        </div>
        <div class="metric-row">
          <span class="metric-label">Started:</span>
          <span class="metric-value">{new Date(props.build.started_at).toLocaleString()}</span>
        </div>
        <div class="metric-row">
          <span class="metric-label">Duration:</span>
          <span class="metric-value">{props.getDuration(props.build)}</span>
        </div>
        <div class="metric-row">
          <span class="metric-label">Status:</span>
          <span class={`metric-value status-${props.build.status}`}>{props.build.status}</span>
        </div>
        <div class="metric-row">
          <span class="metric-label">Targets:</span>
          <span class="metric-value">{props.build.targets.length}</span>
        </div>
        <div class="metric-row">
          <span class="metric-label">Actions:</span>
          <span class="metric-value">{props.build.actions}</span>
        </div>
      </div>
    </div>
  );
};
