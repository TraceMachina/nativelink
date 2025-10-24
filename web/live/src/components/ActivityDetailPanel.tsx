import { Component, createSignal, For, Show } from 'solid-js';
import { TrackedLiveAction, LiveActionMetaKey } from '../client/model';
import './ActivityDetailPanel.css';

type TabType = 'details' | 'stdout' | 'stderr';

export const ActivityDetailPanel: Component<{
  action: TrackedLiveAction;
  onClose: () => void;
}> = (props) => {
  const [activeTab, setActiveTab] = createSignal<TabType>('details');

  const formatTimestamp = (timestamp?: number) => {
    if (!timestamp) return 'N/A';
    return new Date(timestamp).toLocaleString();
  };

  const formatDuration = (start?: number, end?: number) => {
    if (!start || !end) return 'N/A';
    const duration = end - start;
    if (duration < 1000) return `${duration}ms`;
    return `${(duration / 1000).toFixed(2)}s`;
  };

  const getStdoutLength = () => props.action.meta[LiveActionMetaKey.LengthStdout] || 0;
  const getStderrLength = () => props.action.meta[LiveActionMetaKey.LengthStderr] || 0;

  // Generate mock stdout/stderr for demo
  const getMockStdout = () => {
    const lines = [
      'Starting build process...',
      'Compiling source files',
      `[1/10] Building ${props.action.meta['rbe.command'] || 'target'}`,
      '[2/10] Linking dependencies',
      '[3/10] Running tests',
      '✓ All tests passed',
      '[4/10] Generating artifacts',
      '[5/10] Optimizing output',
      'Build completed successfully',
    ];
    return lines.join('\n');
  };

  const getMockStderr = () => {
    if (props.action.status === 'failed') {
      return 'Error: Build failed at linking stage\nundefined reference to `main`\ncollect2: error: ld returned 1 exit status';
    }
    return '';
  };

  const getDetailSections = () => {
    const sections: { title: string; items: { label: string; value: string }[] }[] = [];

    // General section
    sections.push({
      title: 'General',
      items: [
        { label: 'UUID', value: props.action.uuid },
        { label: 'Type', value: props.action.type.toUpperCase() },
        { label: 'Status', value: props.action.status.replace('_', ' ') },
        {
          label: 'Requester',
          value: props.action.meta[LiveActionMetaKey.GeneralFrom] || 'Unknown',
        },
        {
          label: 'Started At',
          value: formatTimestamp(props.action.started_at),
        },
        {
          label: 'Updated At',
          value: formatTimestamp(props.action.updated_at),
        },
      ],
    });

    // RBE section
    if (props.action.type === 'rbe') {
      const rbeItems: { label: string; value: string }[] = [];

      if (props.action.meta[LiveActionMetaKey.RBECommand]) {
        rbeItems.push({
          label: 'Command',
          value: props.action.meta[LiveActionMetaKey.RBECommand],
        });
      }
      if (props.action.meta[LiveActionMetaKey.RBEHostname]) {
        rbeItems.push({
          label: 'Worker',
          value: props.action.meta[LiveActionMetaKey.RBEHostname],
        });
      }
      if (props.action.meta[LiveActionMetaKey.RBEInstanceName]) {
        rbeItems.push({
          label: 'Instance',
          value: props.action.meta[LiveActionMetaKey.RBEInstanceName],
        });
      }
      if (props.action.meta[LiveActionMetaKey.RBEOperationName]) {
        rbeItems.push({
          label: 'Operation',
          value: props.action.meta[LiveActionMetaKey.RBEOperationName],
        });
      }
      if (props.action.meta[LiveActionMetaKey.RBERetryCount] !== undefined) {
        rbeItems.push({
          label: 'Retries',
          value: String(props.action.meta[LiveActionMetaKey.RBERetryCount]),
        });
      }
      if (props.action.meta[LiveActionMetaKey.RBEExitCode] !== undefined) {
        rbeItems.push({
          label: 'Exit Code',
          value: String(props.action.meta[LiveActionMetaKey.RBEExitCode]),
        });
      }

      sections.push({
        title: 'Remote Execution',
        items: rbeItems,
      });

      // Timing section
      const timingItems: { label: string; value: string }[] = [];
      const queued = props.action.meta[LiveActionMetaKey.TimestampQueued];
      const workerStart = props.action.meta[LiveActionMetaKey.TimestampWorkerStart];
      const fetchStart = props.action.meta[LiveActionMetaKey.TimestampFetchStart];
      const fetchCompleted = props.action.meta[LiveActionMetaKey.TimestampFetchCompleted];
      const execStart = props.action.meta[LiveActionMetaKey.TimestampExecStart];
      const execCompleted = props.action.meta[LiveActionMetaKey.TimestampExecCompleted];
      const uploadStart = props.action.meta[LiveActionMetaKey.TimestampUploadStart];
      const uploadCompleted = props.action.meta[LiveActionMetaKey.TimestampUploadCompleted];
      const workerCompleted = props.action.meta[LiveActionMetaKey.TimestampWorkerCompleted];

      if (queued && workerStart) {
        timingItems.push({
          label: 'Queue Time',
          value: formatDuration(queued, workerStart),
        });
      }
      if (fetchStart && fetchCompleted) {
        timingItems.push({
          label: 'Fetch Duration',
          value: formatDuration(fetchStart, fetchCompleted),
        });
      }
      if (execStart && execCompleted) {
        timingItems.push({
          label: 'Execution Duration',
          value: formatDuration(execStart, execCompleted),
        });
      }
      if (uploadStart && uploadCompleted) {
        timingItems.push({
          label: 'Upload Duration',
          value: formatDuration(uploadStart, uploadCompleted),
        });
      }
      if (queued && workerCompleted) {
        timingItems.push({
          label: 'Total Duration',
          value: formatDuration(queued, workerCompleted),
        });
      }

      if (timingItems.length > 0) {
        sections.push({
          title: 'Timing',
          items: timingItems,
        });
      }
    }

    return sections;
  };

  return (
    <div class="activity-detail-panel">
      <div class="panel-header">
        <div class="panel-title">
          <span class="material-symbols-outlined">info</span>
          <h3>
            {props.action.type.toUpperCase()} {props.action.uuid.substring(0, 8)}
          </h3>
          <span class={`status-badge ${props.action.status}`}>
            {props.action.status.replace('_', ' ')}
          </span>
        </div>
        <button class="close-button" onClick={props.onClose}>
          <span class="material-symbols-outlined">close</span>
        </button>
      </div>

      <div class="panel-tabs">
        <button
          class="tab"
          classList={{ active: activeTab() === 'details' }}
          onClick={() => setActiveTab('details')}
        >
          Details
        </button>
        <button
          class="tab"
          classList={{ active: activeTab() === 'stdout' }}
          onClick={() => setActiveTab('stdout')}
        >
          Stdout
          <Show when={getStdoutLength() > 0}>
            <span class="badge">{getStdoutLength()}</span>
          </Show>
        </button>
        <button
          class="tab"
          classList={{ active: activeTab() === 'stderr' }}
          onClick={() => setActiveTab('stderr')}
        >
          Stderr
          <Show when={getStderrLength() > 0}>
            <span class="badge error">{getStderrLength()}</span>
          </Show>
        </button>
      </div>

      <div class="panel-content">
        <Show when={activeTab() === 'details'}>
          <div class="details-view">
            <For each={getDetailSections()}>
              {(section) => (
                <div class="detail-section">
                  <h4>{section.title}</h4>
                  <div class="detail-grid">
                    <For each={section.items}>
                      {(item) => (
                        <>
                          <div class="detail-label">{item.label}</div>
                          <div class="detail-value">{item.value}</div>
                        </>
                      )}
                    </For>
                  </div>
                </div>
              )}
            </For>
          </div>
        </Show>

        <Show when={activeTab() === 'stdout'}>
          <div class="log-view">
            <pre>{getMockStdout()}</pre>
          </div>
        </Show>

        <Show when={activeTab() === 'stderr'}>
          <div class="log-view">
            <Show
              when={getMockStderr()}
              fallback={<div class="empty-log">No stderr output</div>}
            >
              <pre class="error-log">{getMockStderr()}</pre>
            </Show>
          </div>
        </Show>
      </div>
    </div>
  );
};
