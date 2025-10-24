/**
 * Handshake response from nativelink server.
 */
export interface NativelinkHandshake {
  /**
   * The version of nativelink server.
   */
  version: string;
  /**
   * Whether `nativelink-live` is supported.
   * If not found or false, not supported.
   */
  live_supported?: boolean;
}

export interface NativelinkState {
  version: string;

  received_count: {
    live?: number;
    workers?: number;
    stores?: number;
    clients?: number;
    logs?: number;
  }
}

export interface TrackedLiveAction {
  uuid: string;
  meta: Record<LiveActionMetaKey, any>;
  type: "upload" | "download" | "rbe" | "clients" | "other";
  status: "pending" | "in_progress" | "completed" | "failed" | "canceled";
  started_at: number; // timestamp
  updated_at: number; // timestamp
}

export interface WorkerState {
  worker_id: string;
  status: 'idle' | 'working' | 'offline';
  last_seen: number;
  platform_properties: Record<string, string[]>;
}

export interface ClientState {
  client_id: string;
  remote_addr: string;
  first_seen: number;
  last_seen: number;
  activity_count: number;
}

export interface LogLine {
  timestamp: number;
  level: string;
  message: string;
  raw: string;
}

export interface BuildInvocation {
  invocation_id: string;
  started_at: number;
  completed_at?: number;
  tool_name: string;
  tool_version: string;
  status: 'running' | 'succeeded' | 'failed';
  targets: string[];
  actions: number;
  correlated_invocations_id: string;
}

// TODO(ilsubyeega): fill out this
export enum LiveActionMetaKey {
  /**
   * The requester's network address.
   */
  GeneralFrom = "general.from",
  RBEAction = "rbe.action",
  RBECommand = "rbe.command",
  RBEExitCode = "rbe.exit_code",
  RBEHostname = "rbe.hostname",
  RBEInstanceName = "rbe.instance_name",
  RBEOperationName = "rbe.operation_name",
  RBERetryCount = "rbe.retry_count",

  // Reuses `ExecutionMetadata` struct
  TimestampQueued = "timestamp.queued",
  TimestampWorkerStart = "timestamp.worker_start",
  TimestampWorkerCompleted = "timestamp.worker_completed",
  TimestampFetchStart = "timestamp.fetch_start",
  TimestampFetchCompleted = "timestamp.fetch_completed",
  TimestampExecStart = "timestamp.exec_start",
  TimestampExecCompleted = "timestamp.exec_completed",
  TimestampUploadStart = "timestamp.upload_start",
  TimestampUploadCompleted = "timestamp.upload_completed",

  LengthStdout = "length.stdout",
  LengthStderr = "length.stderr",
}
