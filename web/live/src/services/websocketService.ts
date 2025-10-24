import { TrackedLiveAction, WorkerState, ClientState, LogLine, BuildInvocation } from '../client/model';

const WS_URL = 'ws://localhost:3002';

export class WebSocketService {
  private ws: WebSocket | null = null;
  private listeners: Set<(actions: TrackedLiveAction[]) => void> = new Set();
  private workerListeners: Set<(workers: WorkerState[]) => void> = new Set();
  private clientListeners: Set<(clients: ClientState[]) => void> = new Set();
  private logListeners: Set<(logs: LogLine[]) => void> = new Set();
  private buildListeners: Set<(builds: BuildInvocation[]) => void> = new Set();
  private reconnectTimeout: number | null = null;
  private actions: TrackedLiveAction[] = [];
  private workers: WorkerState[] = [];
  private clients: ClientState[] = [];
  private logs: LogLine[] = [];
  private builds: BuildInvocation[] = [];

  connect() {
    try {
      this.ws = new WebSocket(WS_URL);

      this.ws.onopen = () => {
        console.log('✅ Connected to NativeLink Live backend');
        if (this.reconnectTimeout) {
          clearTimeout(this.reconnectTimeout);
          this.reconnectTimeout = null;
        }
      };

      this.ws.onmessage = (event) => {
        try {
          const message = JSON.parse(event.data);
          this.handleMessage(message);
        } catch (error) {
          console.error('Failed to parse WebSocket message:', error);
        }
      };

      this.ws.onerror = (error) => {
        console.error('WebSocket error:', error);
      };

      this.ws.onclose = () => {
        console.warn('❌ Disconnected from backend, reconnecting...');
        this.reconnect();
      };
    } catch (error) {
      console.error('Failed to connect to WebSocket:', error);
      this.reconnect();
    }
  }

  private reconnect() {
    if (this.reconnectTimeout) return;

    this.reconnectTimeout = window.setTimeout(() => {
      console.log('🔄 Reconnecting to backend...');
      this.connect();
    }, 2000);
  }

  private handleMessage(message: any) {
    switch (message.type) {
      case 'init':
        this.actions = message.activities || [];
        this.workers = message.workers || [];
        this.clients = message.clients || [];
        this.logs = message.logs || [];
        this.builds = message.builds || [];
        this.notifyListeners();
        this.notifyWorkerListeners();
        this.notifyClientListeners();
        this.notifyLogListeners();
        this.notifyBuildListeners();
        break;

      case 'activity_added':
        if (message.activity) {
          this.actions.unshift(message.activity);
          if (this.actions.length > 100) {
            this.actions = this.actions.slice(0, 100);
          }
          this.notifyListeners();
        }
        break;

      case 'activity_updated':
        if (message.activity) {
          const index = this.actions.findIndex((a) => a.uuid === message.activity.uuid);
          if (index !== -1) {
            this.actions[index] = message.activity;
            this.notifyListeners();
          }
        }
        break;

      case 'workers_updated':
        if (message.workers) {
          this.workers = message.workers;
          this.notifyWorkerListeners();
        }
        break;

      case 'clients_updated':
        if (message.clients) {
          this.clients = message.clients;
          this.notifyClientListeners();
        }
        break;

      case 'log_line':
        if (message.log) {
          this.logs.push(message.log);
          if (this.logs.length > 1000) {
            this.logs.shift(); // Remove oldest log
          }
          this.notifyLogListeners();
        }
        break;

      case 'builds_updated':
        if (message.builds) {
          this.builds = message.builds;
          this.notifyBuildListeners();
        }
        break;

      default:
        console.warn('Unknown message type:', message.type);
    }
  }

  subscribe(callback: (actions: TrackedLiveAction[]) => void) {
    this.listeners.add(callback);
    // Immediately send current state
    callback(this.actions);

    return () => {
      this.listeners.delete(callback);
    };
  }

  private notifyListeners() {
    this.listeners.forEach((callback) => callback([...this.actions]));
  }

  private notifyWorkerListeners() {
    this.workerListeners.forEach((callback) => callback([...this.workers]));
  }

  private notifyClientListeners() {
    this.clientListeners.forEach((callback) => callback([...this.clients]));
  }

  private notifyLogListeners() {
    this.logListeners.forEach((callback) => callback([...this.logs]));
  }

  private notifyBuildListeners() {
    this.buildListeners.forEach((callback) => callback([...this.builds]));
  }

  subscribeWorkers(callback: (workers: WorkerState[]) => void) {
    this.workerListeners.add(callback);
    callback(this.workers);
    return () => {
      this.workerListeners.delete(callback);
    };
  }

  subscribeClients(callback: (clients: ClientState[]) => void) {
    this.clientListeners.add(callback);
    callback(this.clients);
    return () => {
      this.clientListeners.delete(callback);
    };
  }

  subscribeLogs(callback: (logs: LogLine[]) => void) {
    this.logListeners.add(callback);
    callback(this.logs);
    return () => {
      this.logListeners.delete(callback);
    };
  }

  subscribeBuilds(callback: (builds: BuildInvocation[]) => void) {
    this.buildListeners.add(callback);
    callback(this.builds);
    return () => {
      this.buildListeners.delete(callback);
    };
  }

  disconnect() {
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
    if (this.reconnectTimeout) {
      clearTimeout(this.reconnectTimeout);
      this.reconnectTimeout = null;
    }
  }

  getActions() {
    return [...this.actions];
  }

  getWorkers() {
    return [...this.workers];
  }

  getClients() {
    return [...this.clients];
  }

  getLogs() {
    return [...this.logs];
  }

  getBuilds() {
    return [...this.builds];
  }
}
