import { Component, createSignal, For, onMount, onCleanup, Show } from 'solid-js';
import { ClientState } from '../client/model';
import { WebSocketService } from '../services/websocketService';
import './ClientConnectionOverview.css';

const wsService = new WebSocketService();

export const ClientConnectionOverview: Component = () => {
  const [clients, setClients] = createSignal<ClientState[]>([]);

  onMount(() => {
    wsService.connect();
    const unsubscribe = wsService.subscribeClients(setClients);

    onCleanup(() => {
      unsubscribe();
      wsService.disconnect();
    });
  });

  const formatTimestamp = (timestamp: number) => {
    return new Date(timestamp).toLocaleString();
  };

  const getTimeAgo = (timestamp: number) => {
    const seconds = Math.floor((Date.now() - timestamp) / 1000);
    if (seconds < 60) return `${seconds}s ago`;
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
    if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
    return `${Math.floor(seconds / 86400)}d ago`;
  };

  const formatDuration = (firstSeen: number, lastSeen: number) => {
    const seconds = Math.floor((lastSeen - firstSeen) / 1000);
    if (seconds < 60) return `${seconds}s`;
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
    if (seconds < 86400) return `${Math.floor(seconds / 3600)}h`;
    return `${Math.floor(seconds / 86400)}d`;
  };

  return (
    <div class="client-overview">
      <div class="overview-header">
        <h2>
          <span class="material-symbols-outlined">devices</span>
          Client Connection Overview
        </h2>
        <div class="client-summary">
          <div class="summary-card">
            <span class="summary-label">Total Clients</span>
            <span class="summary-value">{clients().length}</span>
          </div>
          <div class="summary-card">
            <span class="summary-label">Total Activity</span>
            <span class="summary-value">
              {clients().reduce((sum, c) => sum + c.activity_count, 0)}
            </span>
          </div>
        </div>
      </div>

      <Show
        when={clients().length > 0}
        fallback={
          <div class="empty-state">
            <span class="material-symbols-outlined">info</span>
            <p>No clients detected yet. Clients will appear here when they connect to NativeLink.</p>
            <p class="hint">Try running a Bazel build to see client connections.</p>
          </div>
        }
      >
        <div class="clients-table">
          <table>
            <thead>
              <tr>
                <th>Remote Address</th>
                <th>First Seen</th>
                <th>Last Seen</th>
                <th>Duration</th>
                <th>Activity Count</th>
              </tr>
            </thead>
            <tbody>
              <For each={clients()}>
                {(client) => (
                  <tr>
                    <td>
                      <code class="client-addr">{client.remote_addr}</code>
                    </td>
                    <td>
                      <span class="timestamp" title={formatTimestamp(client.first_seen)}>
                        {getTimeAgo(client.first_seen)}
                      </span>
                    </td>
                    <td>
                      <span class="timestamp" title={formatTimestamp(client.last_seen)}>
                        {getTimeAgo(client.last_seen)}
                      </span>
                    </td>
                    <td>
                      <span class="duration">
                        {formatDuration(client.first_seen, client.last_seen)}
                      </span>
                    </td>
                    <td>
                      <span class="activity-badge">{client.activity_count}</span>
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
