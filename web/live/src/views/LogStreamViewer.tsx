import { Component, createSignal, For, onMount, onCleanup, Show, createEffect, createMemo } from 'solid-js';
import { LogLine } from '../client/model';
import { WebSocketService } from '../services/websocketService';
import './LogStreamViewer.css';

const wsService = new WebSocketService();

export const LogStreamViewer: Component = () => {
  const [logs, setLogs] = createSignal<LogLine[]>([]);
  const [filterLevel, setFilterLevel] = createSignal<string>('ALL');
  const [searchQuery, setSearchQuery] = createSignal('');
  const [autoScroll, setAutoScroll] = createSignal(true);
  let logContainerRef: HTMLDivElement | undefined;

  onMount(() => {
    wsService.connect();
    const unsubscribe = wsService.subscribeLogs(setLogs);

    onCleanup(() => {
      unsubscribe();
      wsService.disconnect();
    });
  });

  // Auto-scroll to bottom when new logs arrive
  createEffect(() => {
    if (autoScroll() && logContainerRef) {
      logContainerRef.scrollTop = logContainerRef.scrollHeight;
    }
  });

  const filteredLogs = createMemo(() => {
    let filtered = logs();

    // Filter by level
    if (filterLevel() !== 'ALL') {
      filtered = filtered.filter((log) => log.level === filterLevel());
    }

    // Filter by search query
    const query = searchQuery().toLowerCase();
    if (query) {
      filtered = filtered.filter((log) => log.message.toLowerCase().includes(query));
    }

    return filtered;
  });

  const formatTimestamp = (timestamp: number) => {
    const date = new Date(timestamp);
    return date.toLocaleTimeString('en-US', {
      hour12: false,
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
      fractionalSecondDigits: 3
    });
  };

  const getLevelClass = (level: string) => {
    switch (level) {
      case 'ERROR':
        return 'log-level-error';
      case 'WARN':
        return 'log-level-warn';
      case 'INFO':
        return 'log-level-info';
      case 'DEBUG':
        return 'log-level-debug';
      case 'TRACE':
        return 'log-level-trace';
      default:
        return 'log-level-default';
    }
  };

  const handleScroll = (e: Event) => {
    const target = e.target as HTMLDivElement;
    const isAtBottom = target.scrollHeight - target.scrollTop <= target.clientHeight + 50;
    setAutoScroll(isAtBottom);
  };

  const clearLogs = () => {
    setLogs([]);
  };

  const logCounts = () => {
    const counts = { ALL: logs().length, INFO: 0, DEBUG: 0, WARN: 0, ERROR: 0, TRACE: 0 };
    logs().forEach((log) => {
      if (counts[log.level as keyof typeof counts] !== undefined) {
        counts[log.level as keyof typeof counts]++;
      }
    });
    return counts;
  };

  return (
    <div class="log-viewer">
      <div class="log-header">
        <h2>
          <span class="material-symbols-outlined">terminal</span>
          Log Stream Viewer
        </h2>
        <div class="log-controls">
          <div class="log-stats">
            <span class="stat-item">Total: {logCounts().ALL}</span>
          </div>
          <div class="filter-buttons">
            <button
              class={filterLevel() === 'ALL' ? 'filter-btn active' : 'filter-btn'}
              onClick={() => setFilterLevel('ALL')}
            >
              ALL ({logCounts().ALL})
            </button>
            <button
              class={filterLevel() === 'INFO' ? 'filter-btn filter-info active' : 'filter-btn filter-info'}
              onClick={() => setFilterLevel('INFO')}
            >
              INFO ({logCounts().INFO})
            </button>
            <button
              class={filterLevel() === 'DEBUG' ? 'filter-btn filter-debug active' : 'filter-btn filter-debug'}
              onClick={() => setFilterLevel('DEBUG')}
            >
              DEBUG ({logCounts().DEBUG})
            </button>
            <button
              class={filterLevel() === 'WARN' ? 'filter-btn filter-warn active' : 'filter-btn filter-warn'}
              onClick={() => setFilterLevel('WARN')}
            >
              WARN ({logCounts().WARN})
            </button>
            <button
              class={filterLevel() === 'ERROR' ? 'filter-btn filter-error active' : 'filter-btn filter-error'}
              onClick={() => setFilterLevel('ERROR')}
            >
              ERROR ({logCounts().ERROR})
            </button>
          </div>
          <input
            type="text"
            class="search-input"
            placeholder="Search logs..."
            value={searchQuery()}
            onInput={(e) => setSearchQuery(e.currentTarget.value)}
          />
          <button class="clear-btn" onClick={clearLogs} title="Clear logs">
            <span class="material-symbols-outlined">delete</span>
          </button>
          <div class="auto-scroll-toggle">
            <label>
              <input
                type="checkbox"
                checked={autoScroll()}
                onChange={(e) => setAutoScroll(e.currentTarget.checked)}
              />
              Auto-scroll
            </label>
          </div>
        </div>
      </div>

      <div class="log-container" ref={logContainerRef} onScroll={handleScroll}>
        <Show
          when={filteredLogs().length > 0}
          fallback={
            <div class="empty-logs">
              <span class="material-symbols-outlined">info</span>
              <p>
                {logs().length === 0
                  ? 'Waiting for logs... Make sure NativeLink is running with DEBUG logging.'
                  : 'No logs match the current filters.'}
              </p>
            </div>
          }
        >
          <div class="log-lines">
            <For each={filteredLogs()}>
              {(log) => (
                <div class={`log-line ${getLevelClass(log.level)}`}>
                  <span class="log-timestamp">{formatTimestamp(log.timestamp)}</span>
                  <span class={`log-level ${getLevelClass(log.level)}`}>{log.level}</span>
                  <span class="log-message">{log.message}</span>
                </div>
              )}
            </For>
          </div>
        </Show>
      </div>
    </div>
  );
};
