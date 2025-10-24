import { TrackedLiveAction, LiveActionMetaKey } from '../client/model';

// Generate mock UUIDs
const generateUUID = () => {
  return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    const v = c === 'x' ? r : (r & 0x3) | 0x8;
    return v.toString(16);
  });
};

const types: Array<'upload' | 'download' | 'rbe' | 'other'> = ['upload', 'download', 'rbe', 'other'];
const statuses: Array<'pending' | 'in_progress' | 'completed' | 'failed' | 'canceled'> = [
  'pending',
  'in_progress',
  'completed',
  'failed',
  'canceled',
];

const sampleCommands = [
  'rustc --crate-name nativelink_worker',
  'cargo build --release',
  'bazel build //src:main',
  'gcc -O2 -o output main.c',
  'npm run build',
  'go build ./...',
];

const sampleHosts = ['worker-01', 'worker-02', 'worker-03', 'worker-04', 'worker-05'];
const sampleIPs = ['192.168.1.10', '192.168.1.11', '192.168.1.12', '10.0.0.5', '10.0.0.6'];

export function generateMockAction(): TrackedLiveAction {
  const type = types[Math.floor(Math.random() * types.length)];
  const status = statuses[Math.floor(Math.random() * statuses.length)];
  const now = Date.now();
  const startedAt = now - Math.random() * 300000; // Started within last 5 minutes

  const meta: Record<LiveActionMetaKey, any> = {
    [LiveActionMetaKey.GeneralFrom]: sampleIPs[Math.floor(Math.random() * sampleIPs.length)],
  };

  if (type === 'rbe') {
    meta[LiveActionMetaKey.RBECommand] = sampleCommands[Math.floor(Math.random() * sampleCommands.length)];
    meta[LiveActionMetaKey.RBEHostname] = sampleHosts[Math.floor(Math.random() * sampleHosts.length)];
    meta[LiveActionMetaKey.RBEInstanceName] = 'main';
    meta[LiveActionMetaKey.RBEOperationName] = `operations/${generateUUID()}`;
    meta[LiveActionMetaKey.RBERetryCount] = Math.floor(Math.random() * 3);

    if (status === 'completed' || status === 'failed') {
      meta[LiveActionMetaKey.RBEExitCode] = status === 'completed' ? 0 : Math.floor(Math.random() * 10) + 1;
      meta[LiveActionMetaKey.LengthStdout] = Math.floor(Math.random() * 10000);
      meta[LiveActionMetaKey.LengthStderr] = status === 'failed' ? Math.floor(Math.random() * 1000) : 0;
    }

    // Add execution metadata
    meta[LiveActionMetaKey.TimestampQueued] = startedAt;
    if (status !== 'pending') {
      meta[LiveActionMetaKey.TimestampWorkerStart] = startedAt + 1000;
      meta[LiveActionMetaKey.TimestampFetchStart] = startedAt + 2000;
      meta[LiveActionMetaKey.TimestampFetchCompleted] = startedAt + 5000;
      meta[LiveActionMetaKey.TimestampExecStart] = startedAt + 5500;
      if (status === 'completed' || status === 'failed') {
        meta[LiveActionMetaKey.TimestampExecCompleted] = startedAt + 15000;
        meta[LiveActionMetaKey.TimestampUploadStart] = startedAt + 15500;
        meta[LiveActionMetaKey.TimestampUploadCompleted] = startedAt + 18000;
        meta[LiveActionMetaKey.TimestampWorkerCompleted] = startedAt + 18500;
      }
    }
  }

  return {
    uuid: generateUUID(),
    type,
    status,
    meta,
    started_at: startedAt,
    updated_at: now,
  };
}

export function generateMockActions(count: number): TrackedLiveAction[] {
  return Array.from({ length: count }, () => generateMockAction());
}

// Simulated real-time updates
export class MockDataService {
  private actions: TrackedLiveAction[] = [];
  private listeners: Set<(actions: TrackedLiveAction[]) => void> = new Set();
  private updateInterval: number | null = null;

  constructor(initialCount = 20) {
    this.actions = generateMockActions(initialCount);
  }

  subscribe(callback: (actions: TrackedLiveAction[]) => void) {
    this.listeners.add(callback);
    callback(this.actions);
    return () => this.listeners.delete(callback);
  }

  startSimulation() {
    if (this.updateInterval !== null) return;

    this.updateInterval = window.setInterval(() => {
      // Randomly update existing actions
      this.actions = this.actions.map((action) => {
        if (action.status === 'pending' && Math.random() > 0.7) {
          return { ...action, status: 'in_progress', updated_at: Date.now() };
        }
        if (action.status === 'in_progress' && Math.random() > 0.8) {
          const newStatus = Math.random() > 0.2 ? 'completed' : 'failed';
          return { ...action, status: newStatus, updated_at: Date.now() };
        }
        return action;
      });

      // Add new actions occasionally
      if (Math.random() > 0.6) {
        this.actions.unshift(generateMockAction());
        // Keep only last 100 actions
        if (this.actions.length > 100) {
          this.actions = this.actions.slice(0, 100);
        }
      }

      this.notifyListeners();
    }, 2000);
  }

  stopSimulation() {
    if (this.updateInterval !== null) {
      clearInterval(this.updateInterval);
      this.updateInterval = null;
    }
  }

  private notifyListeners() {
    this.listeners.forEach((callback) => callback([...this.actions]));
  }

  getActions() {
    return [...this.actions];
  }
}
