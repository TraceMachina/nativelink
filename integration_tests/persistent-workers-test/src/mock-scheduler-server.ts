// Copyright 2025 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//

import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';
import * as path from 'path';
import { v4 as uuidv4 } from 'uuid';

interface Worker {
  id: string;
  persistentKey?: string;
  isAlive: boolean;
  requestCount: number;
  lastUsed: Date;
}

export class MockSchedulerServer {
  private server: grpc.Server;
  private workers: Map<string, Worker> = new Map();
  private port: number = 0;

  constructor() {
    this.server = new grpc.Server();
    this.setupServices();
  }

  private setupServices() {
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

    // Implement Execution service
    this.server.addService(proto.build.bazel.remote.execution.v2.Execution.service, {
      Execute: this.handleExecute.bind(this),
      WaitExecution: this.handleWaitExecution.bind(this)
    });

    // Implement Capabilities service
    this.server.addService(proto.build.bazel.remote.execution.v2.Capabilities.service, {
      GetCapabilities: this.handleGetCapabilities.bind(this)
    });
  }

  private handleExecute(call: grpc.ServerWritableStream<any, any>) {
    const request = call.request;
    console.log('Execute request received:', request);

    // Extract platform properties
    const platform = request.action?.platform;
    let persistentKey: string | undefined;

    if (platform?.properties) {
      const keyProp = platform.properties.find((p: any) => p.name === 'persistentWorkerKey');
      persistentKey = keyProp?.value;
    }

    // Simulate worker spawning if needed
    if (persistentKey && !this.hasWorkerForKey(persistentKey)) {
      this.spawnWorker(persistentKey);
    }

    // Simulate execution
    setTimeout(() => {
      // Send initial response
      call.write({
        name: `operations/${uuidv4()}`,
        metadata: {
          '@type': 'type.googleapis.com/build.bazel.remote.execution.v2.ExecuteOperationMetadata',
          stage: 'EXECUTING',
          action_digest: request.action_digest
        },
        done: false
      });

      // Send completion after delay
      setTimeout(() => {
        const worker = persistentKey ? this.getWorkerForKey(persistentKey) : null;
        if (worker) {
          worker.requestCount++;
          worker.lastUsed = new Date();
        }

        call.write({
          name: `operations/${uuidv4()}`,
          metadata: {
            '@type': 'type.googleapis.com/build.bazel.remote.execution.v2.ExecuteOperationMetadata',
            stage: 'COMPLETED',
            action_digest: request.action_digest
          },
          done: true,
          response: {
            '@type': 'type.googleapis.com/build.bazel.remote.execution.v2.ExecuteResponse',
            result: {
              action_result: {
                stdout_raw: Buffer.from('Mock execution successful'),
                stderr_raw: Buffer.from(''),
                exit_code: 0
              }
            }
          }
        });

        call.end();
      }, 500);
    }, 100);
  }

  private handleWaitExecution(call: grpc.ServerWritableStream<any, any>) {
    const request = call.request;
    console.log('WaitExecution request received:', request);

    // Send immediate response for simplicity
    call.write({
      name: request.name,
      done: true,
      response: {
        '@type': 'type.googleapis.com/build.bazel.remote.execution.v2.ExecuteResponse',
        result: {
          action_result: {
            stdout_raw: Buffer.from('Wait execution completed'),
            stderr_raw: Buffer.from(''),
            exit_code: 0
          }
        }
      }
    });

    call.end();
  }

  private handleGetCapabilities(
    call: grpc.ServerUnaryCall<any, any>,
    callback: grpc.sendUnaryData<any>
  ) {
    console.log('GetCapabilities request received');

    callback(null, {
      execution_capabilities: {
        digest_function: 'SHA256',
        exec_enabled: true,
        execution_priority_capabilities: {
          priorities: [
            { min_priority: 0, max_priority: 100 }
          ]
        },
        supported_node_properties: ['persistentWorkerKey', 'persistentWorkerTool']
      },
      cache_capabilities: {
        digest_functions: ['SHA256'],
        action_cache_update_capabilities: {
          update_enabled: true
        },
        cache_priority_capabilities: {
          priorities: [
            { min_priority: 0, max_priority: 100 }
          ]
        },
        max_batch_total_size_bytes: '4194304',
        symlink_absolute_path_strategy: 'ALLOWED'
      },
      low_api_version: {
        major: 2,
        minor: 0
      },
      high_api_version: {
        major: 2,
        minor: 3
      }
    });
  }

  private spawnWorker(persistentKey: string): Worker {
    const worker: Worker = {
      id: `worker-${uuidv4()}`,
      persistentKey,
      isAlive: true,
      requestCount: 0,
      lastUsed: new Date()
    };

    this.workers.set(worker.id, worker);
    console.log(`Spawned worker ${worker.id} for persistent key: ${persistentKey}`);
    return worker;
  }

  private hasWorkerForKey(persistentKey: string): boolean {
    return Array.from(this.workers.values()).some(
      w => w.persistentKey === persistentKey && w.isAlive
    );
  }

  private getWorkerForKey(persistentKey: string): Worker | null {
    return Array.from(this.workers.values()).find(
      w => w.persistentKey === persistentKey && w.isAlive
    ) || null;
  }

  async start(port: number): Promise<void> {
    return new Promise((resolve, reject) => {
      this.server.bindAsync(
        `0.0.0.0:${port}`,
        grpc.ServerCredentials.createInsecure(),
        (err, actualPort) => {
          if (err) {
            reject(err);
            return;
          }

          this.port = actualPort;
          this.server.start();
          console.log(`Mock scheduler server started on port ${actualPort}`);
          resolve();
        }
      );
    });
  }

  async stop(): Promise<void> {
    return new Promise((resolve) => {
      this.server.tryShutdown(() => {
        console.log('Mock scheduler server stopped');
        resolve();
      });
    });
  }

  getWorkers(): Worker[] {
    return Array.from(this.workers.values());
  }

  crashWorker(persistentKey: string) {
    const worker = this.getWorkerForKey(persistentKey);
    if (worker) {
      worker.isAlive = false;
      console.log(`Crashed worker ${worker.id} with key: ${persistentKey}`);
    }
  }
}
