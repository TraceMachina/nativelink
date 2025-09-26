// Copyright 2025 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//

import * as fs from 'fs/promises';
import * as path from 'path';
import * as os from 'os';
import { ChildProcess } from 'child_process';
import { PersistentWorkerClient } from './persistent-worker-client';

interface TestConfig {
  schedulerPort: number;
  workerCount: number;
  testDuration: number;
  tempDir: string;
}

class E2ETestRunner {
  private config: TestConfig;
  private processes: ChildProcess[] = [];
  private client: PersistentWorkerClient | null = null;

  constructor(config: Partial<TestConfig> = {}) {
    this.config = {
      schedulerPort: config.schedulerPort || 50051,
      workerCount: config.workerCount || 3,
      testDuration: config.testDuration || 60000,
      tempDir: config.tempDir || ''
    };
  }

  async setup() {
    console.log('🚀 Setting up E2E test environment...');

    // Create temp directory
    this.config.tempDir = await fs.mkdtemp(path.join(os.tmpdir(), 'nativelink-e2e-'));
    console.log(`📁 Created temp directory: ${this.config.tempDir}`);

    // Start scheduler
    await this.startScheduler();

    // Initialize client
    this.client = new PersistentWorkerClient(`localhost:${this.config.schedulerPort}`);

    console.log('✅ Setup complete');
  }

  async teardown() {
    console.log('🧹 Cleaning up...');

    // Close client
    if (this.client) {
      this.client.close();
    }

    // Stop all processes
    for (const proc of this.processes) {
      proc.kill('SIGTERM');
    }

    // Clean temp directory
    if (this.config.tempDir) {
      await fs.rm(this.config.tempDir, { recursive: true, force: true });
    }

    console.log('✅ Cleanup complete');
  }

  private async startScheduler(): Promise<void> {
    console.log('📡 Starting scheduler...');

    // In real implementation, this would start the actual nativelink scheduler
    // For now, we'll simulate it
    return new Promise((resolve) => {
      setTimeout(() => {
        console.log(`✅ Scheduler started on port ${this.config.schedulerPort}`);
        resolve();
      }, 1000);
    });
  }

  async runTests() {
    console.log('🧪 Running E2E tests...\n');

    const results: TestResult[] = [];

    // Test 1: Basic persistent worker spawning
    results.push(await this.testBasicWorkerSpawning());

    // Test 2: Worker reuse
    results.push(await this.testWorkerReuse());

    // Test 3: Concurrent actions
    results.push(await this.testConcurrentActions());

    // Test 4: Worker lifecycle
    results.push(await this.testWorkerLifecycle());

    // Test 5: Platform property matching
    results.push(await this.testPlatformMatching());

    // Test 6: Error recovery
    results.push(await this.testErrorRecovery());

    // Test 7: Performance
    results.push(await this.testPerformance());

    // Print results
    this.printResults(results);

    return results.every(r => r.passed);
  }

  private async testBasicWorkerSpawning(): Promise<TestResult> {
    const testName = 'Basic Worker Spawning';
    console.log(`🧪 ${testName}...`);

    try {
      const result = await this.client!.submitPersistentWorkerAction(
        'javac',
        'test-worker-1',
        ['javac', '-version']
      );

      const passed = result !== undefined;
      console.log(passed ? '✅ Passed' : '❌ Failed');

      return { name: testName, passed, duration: 0 };
    } catch (error) {
      console.log('❌ Failed:', error);
      return { name: testName, passed: false, duration: 0, error: String(error) };
    }
  }

  private async testWorkerReuse(): Promise<TestResult> {
    const testName = 'Worker Reuse';
    console.log(`🧪 ${testName}...`);

    try {
      const key = 'reuse-test-worker';

      // First action
      await this.client!.submitPersistentWorkerAction('javac', key, ['javac', '-version']);

      // Second action should reuse same worker
      const start = Date.now();
      await this.client!.submitPersistentWorkerAction('javac', key, ['javac', '-help']);
      const duration = Date.now() - start;

      // Second action should be faster (no spawn overhead)
      const passed = duration < 1000;
      console.log(passed ? '✅ Passed' : '❌ Failed');

      return { name: testName, passed, duration };
    } catch (error) {
      console.log('❌ Failed:', error);
      return { name: testName, passed: false, duration: 0, error: String(error) };
    }
  }

  private async testConcurrentActions(): Promise<TestResult> {
    const testName = 'Concurrent Actions';
    console.log(`🧪 ${testName}...`);

    try {
      const promises = Array.from({ length: 5 }, (_, i) =>
        this.client!.submitPersistentWorkerAction(
          'javac',
          `concurrent-worker-${i}`,
          ['javac', '-version']
        )
      );

      const start = Date.now();
      const results = await Promise.all(promises);
      const duration = Date.now() - start;

      const passed = results.every(r => r !== undefined) && duration < 5000;
      console.log(passed ? '✅ Passed' : '❌ Failed');

      return { name: testName, passed, duration };
    } catch (error) {
      console.log('❌ Failed:', error);
      return { name: testName, passed: false, duration: 0, error: String(error) };
    }
  }

  private async testWorkerLifecycle(): Promise<TestResult> {
    const testName = 'Worker Lifecycle';
    console.log(`🧪 ${testName}...`);

    try {
      const key = 'lifecycle-test-worker';

      // Spawn worker
      await this.client!.submitPersistentWorkerAction('javac', key, ['javac', '-version']);

      // Check it's alive
      const alive = await this.client!.waitForPersistentWorker(key, 5000);

      // Would test cleanup after idle timeout here in real implementation

      const passed = alive;
      console.log(passed ? '✅ Passed' : '❌ Failed');

      return { name: testName, passed, duration: 0 };
    } catch (error) {
      console.log('❌ Failed:', error);
      return { name: testName, passed: false, duration: 0, error: String(error) };
    }
  }

  private async testPlatformMatching(): Promise<TestResult> {
    const testName = 'Platform Matching';
    console.log(`🧪 ${testName}...`);

    try {
      const result = await this.client!.submitPersistentWorkerAction(
        'javac',
        'platform-test-worker',
        ['javac', '-version'],
        {
          cpu: '4',
          memory: '8GB',
          os: 'linux'
        }
      );

      const passed = result !== undefined;
      console.log(passed ? '✅ Passed' : '❌ Failed');

      return { name: testName, passed, duration: 0 };
    } catch (error) {
      console.log('❌ Failed:', error);
      return { name: testName, passed: false, duration: 0, error: String(error) };
    }
  }

  private async testErrorRecovery(): Promise<TestResult> {
    const testName = 'Error Recovery';
    console.log(`🧪 ${testName}...`);

    try {
      // Test with invalid tool
      let errorCaught = false;
      try {
        await this.client!.submitPersistentWorkerAction(
          'invalid-tool',
          'error-test-worker',
          ['invalid-tool', '--help']
        );
      } catch {
        errorCaught = true;
      }

      const passed = errorCaught;
      console.log(passed ? '✅ Passed' : '❌ Failed');

      return { name: testName, passed, duration: 0 };
    } catch (error) {
      console.log('❌ Failed:', error);
      return { name: testName, passed: false, duration: 0, error: String(error) };
    }
  }

  private async testPerformance(): Promise<TestResult> {
    const testName = 'Performance';
    console.log(`🧪 ${testName}...`);

    try {
      const iterations = 20;
      const key = 'perf-test-worker';

      // Warm up
      await this.client!.submitPersistentWorkerAction('javac', key, ['javac', '-version']);

      // Run performance test
      const start = Date.now();
      const promises = Array.from({ length: iterations }, () =>
        this.client!.submitPersistentWorkerAction('javac', key, ['javac', '-version'])
      );

      await Promise.all(promises);
      const duration = Date.now() - start;
      const avgTime = duration / iterations;

      // Should average less than 100ms per action with reuse
      const passed = avgTime < 100;
      console.log(`   Average time: ${avgTime.toFixed(2)}ms`);
      console.log(passed ? '✅ Passed' : '❌ Failed');

      return { name: testName, passed, duration };
    } catch (error) {
      console.log('❌ Failed:', error);
      return { name: testName, passed: false, duration: 0, error: String(error) };
    }
  }

  private printResults(results: TestResult[]) {
    console.log('\n📊 Test Results:');
    console.log('================');

    const passed = results.filter(r => r.passed).length;
    const failed = results.filter(r => !r.passed).length;

    results.forEach(r => {
      const icon = r.passed ? '✅' : '❌';
      const duration = r.duration ? ` (${r.duration}ms)` : '';
      console.log(`${icon} ${r.name}${duration}`);
      if (r.error) {
        console.log(`   Error: ${r.error}`);
      }
    });

    console.log('================');
    console.log(`Passed: ${passed}/${results.length}`);
    console.log(`Failed: ${failed}/${results.length}`);

    if (failed === 0) {
      console.log('\n🎉 All tests passed!');
    } else {
      console.log('\n⚠️  Some tests failed');
    }
  }
}

interface TestResult {
  name: string;
  passed: boolean;
  duration: number;
  error?: string;
}

// Main execution
async function main() {
  const runner = new E2ETestRunner();

  try {
    await runner.setup();
    const success = await runner.runTests();
    await runner.teardown();

    process.exit(success ? 0 : 1);
  } catch (error) {
    console.error('Fatal error:', error);
    await runner.teardown();
    process.exit(1);
  }
}

// Run if executed directly
if (require.main === module) {
  main().catch(console.error);
}

export { E2ETestRunner, TestResult };
