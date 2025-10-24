import { Component, createSignal, For, onMount, onCleanup, Show } from 'solid-js';
import { WorkerState } from '../client/model';
import { WebSocketService } from '../services/websocketService';
import './WorkerStatusDashboard.css';

const wsService = new WebSocketService();

export const WorkerStatusDashboard: Component = () => {
  const [workers, setWorkers] = createSignal<WorkerState[]>([]);

  onMount(() => {
    wsService.connect();
    const unsubscribe = wsService.subscribeWorkers(setWorkers);

    onCleanup(() => {
      unsubscribe();
      wsService.disconnect();
    });
  });

  const formatTimestamp = (timestamp: number) => {
    return new Date(timestamp).toLocaleTimeString();
  };

  const getTimeAgo = (timestamp: number) => {
    const seconds = Math.floor((Date.now() - timestamp) / 1000);
    if (seconds < 60) return `${seconds}s ago`;
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
    return `${Math.floor(seconds / 3600)}h ago`;
  };

  const getStatusBadgeClass = (status: string) => {
    switch (status) {
      case 'idle':
        return 'status-badge status-idle';
      case 'working':
        return 'status-badge status-working';
      case 'offline':
        return 'status-badge status-offline';
      default:
        return 'status-badge';
    }
  };

  const workerCounts = () => {
    const counts = { idle: 0, working: 0, offline: 0 };
    workers().forEach((w) => {
      counts[w.status]++;
    });
    return counts;
  };

  return (
    <div class="worker-dashboard">
      <div class="dashboard-header">
        <h2>
          <span class="material-symbols-outlined">precision_manufacturing</span>
          Worker Status Dashboard
        </h2>
        <div class="worker-summary">
          <div class="summary-item">
            <span class="summary-label">Total:</span>
            <span class="summary-value">{workers().length}</span>
          </div>
          <div class="summary-item">
            <span class="summary-label">Idle:</span>
            <span class="summary-value status-idle">{workerCounts().idle}</span>
          </div>
          <div class="summary-item">
            <span class="summary-label">Working:</span>
            <span class="summary-value status-working">{workerCounts().working}</span>
          </div>
          <div class="summary-item">
            <span class="summary-label">Offline:</span>
            <span class="summary-value status-offline">{workerCounts().offline}</span>
          </div>
        </div>
      </div>

      <Show
        when={workers().length > 0}
        fallback={
          <div class="empty-state">
            <span class="material-symbols-outlined">info</span>
            <p>No workers detected yet. Workers will appear here once they connect to NativeLink.</p>
          </div>
        }
      >
        <div class="workers-table">
          <table>
            <thead>
              <tr>
                <th>Worker ID</th>
                <th>Status</th>
                <th>Last Seen</th>
                <th>Platform Properties</th>
              </tr>
            </thead>
            <tbody>
              <For each={workers()}>
                {(worker) => (
                  <tr>
                    <td>
                      <code class="worker-id">{worker.worker_id}</code>
                    </td>
                    <td>
                      <span class={getStatusBadgeClass(worker.status)}>
                        {worker.status}
                      </span>
                    </td>
                    <td>
                      <span class="timestamp" title={formatTimestamp(worker.last_seen)}>
                        {getTimeAgo(worker.last_seen)}
                      </span>
                    </td>
                    <td>
                      <div class="platform-props">
                        <Show
                          when={Object.keys(worker.platform_properties).length > 0}
                          fallback={<span class="no-props">No properties</span>}
                        >
                          <For each={Object.entries(worker.platform_properties)}>
                            {([key, values]) => (
                              <div class="prop-item">
                                <span class="prop-key">{key}:</span>
                                <span class="prop-value">{values.join(', ')}</span>
                              </div>
                            )}
                          </For>
                        </Show>
                      </div>
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
