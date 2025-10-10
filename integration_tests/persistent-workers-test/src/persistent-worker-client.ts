import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';
import { v4 as uuidv4 } from 'uuid';
import * as path from 'path';

export interface PersistentWorkerKey {
  tool: string;
  key: string;
  platformProperties: Record<string, string>;
}

export interface ActionInfo {
  actionId: string;
  commandArgs: string[];
  inputFiles: string[];
  outputFiles: string[];
  platform: PlatformProperties;
}

export interface PlatformProperties {
  properties: Array<{ name: string; value: string }>;
}

export interface WorkerStats {
  workerId: string;
  lastUsed: Date;
  requestCount: number;
  isAlive: boolean;
  persistentKey?: string;
}

export class PersistentWorkerClient {
  private client: any;
  private schedulerEndpoint: string;

  constructor(schedulerEndpoint: string) {
    this.schedulerEndpoint = schedulerEndpoint;
    this.initializeClient();
  }

  private initializeClient() {
    const allProtosDir = path.join(__dirname, '../../../', 'nativelink-proto');
    const PROTO_PATH = "build/bazel/remote/execution/v2/remote_execution.proto";

    const packageDefinition = protoLoader.loadSync(PROTO_PATH, {
      includeDirs: [allProtosDir],
      keepCase: true,
      longs: String,
      enums: String,
      defaults: true,
      oneofs: true
    });

    const proto = grpc.loadPackageDefinition(packageDefinition) as any;

    this.client = new proto.build.bazel.remote.execution.v2.Execution(
      this.schedulerEndpoint,
      grpc.credentials.createInsecure()
    );
  }

  /**
   * Submit an action that requires a persistent worker
   */
  async submitPersistentWorkerAction(
    tool: string,
    persistentKey: string,
    commandArgs: string[],
    additionalProps: Record<string, string> = {}
  ): Promise<string> {
    const actionId = uuidv4();

    const platform: PlatformProperties = {
      properties: [
        { name: 'persistentWorkerKey', value: persistentKey },
        { name: 'persistentWorkerTool', value: tool },
        ...Object.entries(additionalProps).map(([name, value]) => ({ name, value }))
      ]
    };

    const action: ActionInfo = {
      actionId,
      commandArgs,
      inputFiles: [],
      outputFiles: [],
      platform
    };

    return this.executeAction(action);
  }

  /**
   * Execute an action through the scheduler
   */
  private async executeAction(action: ActionInfo): Promise<string> {
    return new Promise((resolve, reject) => {
      const request = {
        instance_name: 'main',
        skip_cache_lookup: false,
        action_digest: {
          hash: action.actionId,
          size_bytes: 0
        }
      };

      const call = this.client.Execute(request);

      call.on('data', (response: any) => {
        console.log('Received response:', response);
        if (response.done) {
          resolve(response.result?.action_result?.stdout_raw || '');
        }
      });

      call.on('error', (err: Error) => {
        reject(err);
      });

      call.on('end', () => {
        console.log('Stream ended');
      });
    });
  }

  /**
   * Get statistics about persistent workers
   */
  async getWorkerStats(): Promise<WorkerStats[]> {
    // This would need to be implemented with a custom API endpoint
    // For now, return mock data for testing
    return Promise.resolve([
      {
        workerId: 'worker-1',
        lastUsed: new Date(),
        requestCount: 10,
        isAlive: true,
        persistentKey: 'javac-123'
      }
    ]);
  }

  /**
   * Wait for a persistent worker to be spawned
   */
  async waitForPersistentWorker(persistentKey: string, timeoutMs: number = 30000): Promise<boolean> {
    const startTime = Date.now();

    while (Date.now() - startTime < timeoutMs) {
      const stats = await this.getWorkerStats();
      const found = stats.some(s => s.persistentKey === persistentKey && s.isAlive);

      if (found) {
        return true;
      }

      await new Promise(resolve => setTimeout(resolve, 1000));
    }

    return false;
  }

  /**
   * Clean up resources
   */
  close() {
    grpc.closeClient(this.client);
  }
}
