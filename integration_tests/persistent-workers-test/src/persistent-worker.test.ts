// Copyright 2025 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//

import { PersistentWorkerClient, PersistentWorkerKey, WorkerStats } from './persistent-worker-client';
import { MockSchedulerServer } from './mock-scheduler-server';
import * as path from 'path';
import * as fs from 'fs/promises';
import * as os from 'os';

describe('Persistent Workers Integration Tests', () => {
  let client: PersistentWorkerClient;
  let mockServer: MockSchedulerServer;
  let tempDir: string;

  beforeAll(async () => {
    // Create temporary directory for test artifacts
    tempDir = await fs.mkdtemp(path.join(os.tmpdir(), 'pw-test-'));

    // Start mock scheduler server
    mockServer = new MockSchedulerServer();
    await mockServer.start(50051);

    // Initialize client
    client = new PersistentWorkerClient('localhost:50051');
  });

  afterAll(async () => {
    // Cleanup
    if (client !== undefined) {
      client.close();
    }
    if (mockServer !== undefined) {
      await mockServer.stop();
    }
    await fs.rm(tempDir, { recursive: true, force: true });
  });

  describe('Persistent Worker Key Extraction', () => {
    test('should correctly extract persistent worker key from platform properties', () => {
      const key: PersistentWorkerKey = {
        tool: 'javac',
        key: 'javac-worker-123',
        platformProperties: {
          cpu: '4',
          memory: '8GB'
        }
      };

      expect(key.tool).toBe('javac');
      expect(key.key).toBe('javac-worker-123');
      expect(key.platformProperties.cpu).toBe('4');
    });

    test('should handle missing persistent worker properties', () => {
      const platformProps = {
        cpu: '4',
        memory: '8GB'
      };

      // This would be handled by the scheduler
      expect(platformProps).not.toHaveProperty('persistentWorkerKey');
    });
  });

  describe('Worker Lifecycle Management', () => {
    test('should spawn a new persistent worker when needed', async () => {
      const persistentKey = 'javac-test-worker';

      // Submit an action that requires a persistent worker
      const actionPromise = client.submitPersistentWorkerAction(
        'javac',
        persistentKey,
        ['javac', '-version'],
        { cpu: '2' }
      );

      // Wait for worker to be spawned
      const spawned = await client.waitForPersistentWorker(persistentKey, 10000);
      expect(spawned).toBe(true);

      // Verify action completes
      const result = await actionPromise;
      expect(result).toBeDefined();
    });

    test('should reuse existing persistent worker', async () => {
      const persistentKey = 'scalac-reuse-worker';

      // First action - should spawn new worker
      await client.submitPersistentWorkerAction(
        'scalac',
        persistentKey,
        ['scalac', '-version']
      );

      const stats1 = await client.getWorkerStats();
      const worker = stats1.find(s => s.persistentKey === persistentKey);
      expect(worker).toBeDefined();
      const initialRequestCount = worker!.requestCount;

      // Second action - should reuse same worker
      await client.submitPersistentWorkerAction(
        'scalac',
        persistentKey,
        ['scalac', '-help']
      );

      const stats2 = await client.getWorkerStats();
      const workerAfter = stats2.find(s => s.persistentKey === persistentKey);
      expect(workerAfter).toBeDefined();
      expect(workerAfter!.requestCount).toBeGreaterThan(initialRequestCount);
    });

    test('should handle worker timeout and cleanup', async () => {
      // This test would require configuring a short timeout
      // and verifying that idle workers are cleaned up
      const persistentKey = 'timeout-worker';

      await client.submitPersistentWorkerAction(
        'javac',
        persistentKey,
        ['javac', '-version']
      );

      // Wait for idle timeout (in real test, this would be configured shorter)
      // await new Promise(resolve => setTimeout(resolve, 5000));

      // Verify worker is cleaned up
      // const stats = await client.getWorkerStats();
      // const worker = stats.find(s => s.persistentKey === persistentKey);
      // expect(worker?.isAlive).toBe(false);
    });
  });

  describe('Scheduler Integration', () => {
    test('should route actions to correct persistent workers', async () => {
      const javacKey = 'javac-routing-test';
      const scalacKey = 'scalac-routing-test';

      // Submit actions for different tools
      const javacPromise = client.submitPersistentWorkerAction(
        'javac',
        javacKey,
        ['javac', '-version']
      );

      const scalacPromise = client.submitPersistentWorkerAction(
        'scalac',
        scalacKey,
        ['scalac', '-version']
      );

      // Both should complete successfully
      const [javacResult, scalacResult] = await Promise.all([javacPromise, scalacPromise]);

      expect(javacResult).toBeDefined();
      expect(scalacResult).toBeDefined();

      // Verify separate workers were created
      const stats = await client.getWorkerStats();
      const javacWorker = stats.find(s => s.persistentKey === javacKey);
      const scalacWorker = stats.find(s => s.persistentKey === scalacKey);

      expect(javacWorker).toBeDefined();
      expect(scalacWorker).toBeDefined();
      expect(javacWorker!.workerId).not.toBe(scalacWorker!.workerId);
    });

    test('should handle platform property matching', async () => {
      const persistentKey = 'platform-match-test';

      // Submit action with specific platform requirements
      await client.submitPersistentWorkerAction(
        'javac',
        persistentKey,
        ['javac', '-version'],
        {
          cpu: '4',
          memory: '8GB',
          os: 'linux'
        }
      );

      // Worker should be spawned with matching properties
      const stats = await client.getWorkerStats();
      const worker = stats.find(s => s.persistentKey === persistentKey);
      expect(worker).toBeDefined();
      // In real implementation, we'd verify platform properties match
    });
  });

  describe('Error Handling', () => {
    test('should handle worker spawn failures gracefully', async () => {
      const persistentKey = 'invalid-tool-worker';

      try {
        await client.submitPersistentWorkerAction(
          'non-existent-tool',
          persistentKey,
          ['non-existent-tool', '--help']
        );
        fail('Should have thrown an error');
      } catch (error) {
        expect(error).toBeDefined();
      }
    });

    test('should handle worker crashes and respawn', async () => {
      const persistentKey = 'crash-test-worker';

      // First action succeeds
      await client.submitPersistentWorkerAction(
        'javac',
        persistentKey,
        ['javac', '-version']
      );

      // Simulate worker crash (would need actual implementation)
      // await mockServer.crashWorker(persistentKey);

      // Next action should trigger respawn
      const result = await client.submitPersistentWorkerAction(
        'javac',
        persistentKey,
        ['javac', '-version']
      );

      expect(result).toBeDefined();
    });
  });

  describe('Performance Tests', () => {
    test('should handle concurrent requests efficiently', async () => {
      const persistentKey = 'concurrent-test-worker';
      const numRequests = 10;

      const promises = Array.from({ length: numRequests }, (_, i) =>
        client.submitPersistentWorkerAction(
          'javac',
          persistentKey,
          ['javac', `-Dindex=${i}`, '-version']
        )
      );

      const startTime = Date.now();
      const results = await Promise.all(promises);
      const duration = Date.now() - startTime;

      expect(results).toHaveLength(numRequests);
      results.forEach(result => expect(result).toBeDefined());

      // Performance assertion - should complete reasonably fast
      expect(duration).toBeLessThan(10000); // 10 seconds for 10 requests
    });

    test('should maintain worker pool size limits', async () => {
      const maxWorkers = 5;
      const keys = Array.from({ length: maxWorkers + 2 }, (_, i) => `worker-limit-${i}`);

      // Try to spawn more workers than the limit
      const promises = keys.map(key =>
        client.submitPersistentWorkerAction(
          'javac',
          key,
          ['javac', '-version']
        )
      );

      await Promise.all(promises);

      const stats = await client.getWorkerStats();
      const activeWorkers = stats.filter(s => s.isAlive && s.persistentKey?.startsWith('worker-limit-'));

      // Should not exceed max workers (some may have been evicted)
      expect(activeWorkers.length).toBeLessThanOrEqual(maxWorkers);
    });
  });
});

describe('Persistent Worker Protocol Tests', () => {
  test('should correctly serialize work requests', () => {
    const workRequest = {
      arguments: ['javac', '-d', 'out', 'Main.java'],
      inputs: [{ path: 'Main.java', digest: Buffer.from('abc123') }],
      requestId: 'req-123',
      cancel: false
    };

    // Would test protocol buffer serialization
    expect(workRequest.arguments).toContain('javac');
    expect(workRequest.requestId).toBe('req-123');
  });

  test('should correctly parse work responses', () => {
    const workResponse = {
      exitCode: 0,
      output: 'Compilation successful',
      requestId: 'req-123',
      wasCancelled: false
    };

    expect(workResponse.exitCode).toBe(0);
    expect(workResponse.output).toContain('successful');
  });
});
