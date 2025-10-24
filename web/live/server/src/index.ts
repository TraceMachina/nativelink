import express from 'express';
import { WebSocketServer } from 'ws';
import cors from 'cors';
import http from 'http';
import { spawn } from 'child_process';
import { randomUUID } from 'crypto';
import { existsSync, readFileSync } from 'fs';
import pg from 'pg';

const { Pool } = pg;

// PostgreSQL connection pool
const pool = new Pool({
  connectionString: 'postgresql://tsdbadmin:sdzq6vr4846u3sjz@s4z4dojl1a.jpribe0896.tsdb.cloud.timescale.com:35203/tsdb?sslmode=require',
  max: 20,
  idleTimeoutMillis: 30000,
  connectionTimeoutMillis: 2000,
});

// Initialize database schema
async function initializeDatabase() {
  try {
    // Create logs table as a hypertable
    await pool.query(`
      CREATE TABLE IF NOT EXISTS nativelink_logs (
        id SERIAL,
        timestamp TIMESTAMPTZ NOT NULL,
        level VARCHAR(10) NOT NULL,
        message TEXT NOT NULL,
        raw TEXT NOT NULL,
        PRIMARY KEY (timestamp, id)
      );
    `);

    // Convert to hypertable if not already
    await pool.query(`
      SELECT create_hypertable('nativelink_logs', 'timestamp',
        if_not_exists => TRUE,
        migrate_data => TRUE
      );
    `);

    // Create index on level for faster filtering
    await pool.query(`
      CREATE INDEX IF NOT EXISTS idx_nativelink_logs_level
      ON nativelink_logs (level, timestamp DESC);
    `);

    // Create build invocations table
    await pool.query(`
      CREATE TABLE IF NOT EXISTS nativelink_build_invocations (
        id SERIAL PRIMARY KEY,
        invocation_id VARCHAR(255) UNIQUE NOT NULL,
        started_at TIMESTAMPTZ NOT NULL,
        completed_at TIMESTAMPTZ,
        command TEXT NOT NULL,
        status VARCHAR(50),
        target TEXT,
        flags JSONB,
        meta JSONB
      );
    `);

    // Create index on started_at for time-based queries
    await pool.query(`
      CREATE INDEX IF NOT EXISTS idx_build_invocations_started
      ON nativelink_build_invocations (started_at DESC);
    `);

    console.log('✅ Database schema initialized successfully');
  } catch (error) {
    console.error('❌ Failed to initialize database schema:', error);
    throw error;
  }
}

// Test database connection
pool.on('connect', () => {
  console.log('✅ Connected to PostgreSQL (TimescaleDB)');
});

pool.on('error', (err) => {
  console.error('❌ PostgreSQL connection error:', err);
});

const app = express();
const server = http.createServer(app);
const wss = new WebSocketServer({ server });

app.use(cors());
app.use(express.json());

// Store connected clients
const clients = new Set<any>();

// Mock data structure for NativeLink state
interface WorkerState {
  worker_id: string;
  status: 'idle' | 'working' | 'offline';
  last_seen: number;
  platform_properties: Record<string, string[]>;
}

interface Activity {
  uuid: string;
  type: 'upload' | 'download' | 'rbe' | 'clients' | 'other';
  status: 'pending' | 'in_progress' | 'completed' | 'failed' | 'canceled';
  started_at: number;
  updated_at: number;
  meta: Record<string, any>;
}

// Log parsing patterns - Updated to match actual NativeLink log format
const LOG_PATTERNS = {
  clientConnected: /Client connected.*remote_addr[:\s]+([0-9.:]+)/,
  workerRegistered: /Worker registered.*worker_id[:\s]+([a-f0-9-]+)/,
  workerTimeout: /Worker timed out.*worker_id[:\s]+([a-f0-9-]+)/,
  getCapabilities: /get_capabilities.*instance_name[:\s]+"([^"]+)"/,
  actionExecute: /nativelink_scheduler::action_scheduler.*execute/i,
  uploadBlob: /bytestream_server.*inner_write|uploads\//,
  downloadBlob: /bytestream_server.*read|downloads\//,
  findMissingBlobs: /FindMissingBlobs|find_missing_blobs/,
  // Filesystem store patterns (DEBUG level logs)
  filesystemRenamed: /nativelink_store::filesystem_store.*Renamed file.*key: Digest\(DigestInfo\("([a-f0-9]+-\d+)"\)\)/,
  filesystemDeleted: /nativelink_store::filesystem_store.*File deleted.*file_path: "([^"]+)"/,
  filesystemSpawnDelete: /nativelink_store::filesystem_store.*Spawned a filesystem_delete_file.*current_active_drop_spawns: (\d+)/,
  // Build invocation patterns
  bazelMetadata: /Bazel request metadata: RequestMetadata \{ tool_details: Some\(ToolDetails \{ tool_name: "([^"]+)", tool_version: "([^"]+)" \}\), action_id: "([^"]*)", tool_invocation_id: "([^"]+)", correlated_invocations_id: "([^"]+)", action_mnemonic: "([^"]*)", target_id: "([^"]*)",/,
  // Error patterns that might indicate build failure
  actionError: /ERROR.*tool_invocation_id[:\s]+"([^"]+)"/,
  executionError: /ERROR.*correlated_invocations_id[:\s]+"([^"]+)"/,
};

interface ClientState {
  client_id: string;
  remote_addr: string;
  first_seen: number;
  last_seen: number;
  activity_count: number;
}

interface LogLine {
  timestamp: number;
  level: string;
  message: string;
  raw: string;
}

interface BuildInvocation {
  invocation_id: string;
  started_at: number;
  completed_at?: number;
  tool_name: string;
  tool_version: string;
  status: 'running' | 'succeeded' | 'failed';
  targets: Set<string>;
  actions: number;
  correlated_invocations_id: string;
  last_activity: number;
}

const state = {
  workers: new Map<string, WorkerState>(),
  clients: new Map<string, ClientState>(),
  logs: [] as LogLine[],
  builds: new Map<string, BuildInvocation>(),
  lastUpdate: Date.now(),
};

const MAX_LOGS = 1000;

// Fetch NativeLink health status
async function fetchNativelinkHealth() {
  try {
    const response = await fetch('http://localhost:50061/status');
    if (response.ok) {
      const data = await response.json();
      console.log('NativeLink health:', data);
      return data;
    }
  } catch (error) {
    console.error('Failed to fetch NativeLink health:', error);
  }
  return null;
}

// Parse log line and update state (clients, workers, logs)
async function processLogLine(line: string) {
  // Remove ANSI color codes
  const cleanLine = line.replace(/\x1b\[[0-9;]*m/g, '');

  // Extract timestamp
  const timestampMatch = cleanLine.match(/(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z)/);
  const timestamp = timestampMatch ? new Date(timestampMatch[1]).getTime() : Date.now();

  // Extract log level
  const levelMatch = cleanLine.match(/\s+(INFO|DEBUG|WARN|ERROR|TRACE)\s+/);
  const level = levelMatch ? levelMatch[1] : 'INFO';

  // Store log line
  const logLine: LogLine = {
    timestamp,
    level,
    message: cleanLine,
    raw: line,
  };

  // Insert into database (async, non-blocking)
  pool.query(
    'INSERT INTO nativelink_logs (timestamp, level, message, raw) VALUES (to_timestamp($1 / 1000.0), $2, $3, $4)',
    [timestamp, level, cleanLine, line]
  ).catch((err) => {
    console.error('Failed to insert log into database:', err);
  });

  state.logs.push(logLine);
  if (state.logs.length > MAX_LOGS) {
    state.logs.shift(); // Remove oldest log
  }

  // Broadcast new log line
  broadcast({
    type: 'log_line',
    log: logLine,
  });

  // Track client connections
  if (LOG_PATTERNS.clientConnected.test(cleanLine)) {
    const match = cleanLine.match(LOG_PATTERNS.clientConnected);
    const remoteAddr = match?.[1] || 'unknown';
    const clientId = remoteAddr;

    const existingClient = state.clients.get(clientId);
    if (existingClient) {
      existingClient.last_seen = timestamp;
      existingClient.activity_count++;
    } else {
      state.clients.set(clientId, {
        client_id: clientId,
        remote_addr: remoteAddr,
        first_seen: timestamp,
        last_seen: timestamp,
        activity_count: 1,
      });
    }

    broadcast({
      type: 'clients_updated',
      clients: Array.from(state.clients.values()),
    });
  }

  // Track worker registrations
  if (LOG_PATTERNS.workerRegistered.test(cleanLine)) {
    const match = cleanLine.match(LOG_PATTERNS.workerRegistered);
    const workerId = match?.[1] || '';

    state.workers.set(workerId, {
      worker_id: workerId,
      status: 'idle',
      last_seen: timestamp,
      platform_properties: {},
    });

    broadcast({
      type: 'workers_updated',
      workers: Array.from(state.workers.values()),
    });
  }

  // Track worker timeouts
  if (LOG_PATTERNS.workerTimeout.test(cleanLine)) {
    const match = cleanLine.match(LOG_PATTERNS.workerTimeout);
    const workerId = match?.[1] || '';

    const worker = state.workers.get(workerId);
    if (worker) {
      worker.status = 'offline';
      worker.last_seen = timestamp;
    }

    broadcast({
      type: 'workers_updated',
      workers: Array.from(state.workers.values()),
    });
  }

  // Track build invocations from Bazel metadata
  if (LOG_PATTERNS.bazelMetadata.test(cleanLine)) {
    const match = cleanLine.match(LOG_PATTERNS.bazelMetadata);
    if (match) {
      const [, toolName, toolVersion, actionId, toolInvocationId, correlatedInvocationsId, actionMnemonic, targetId] = match;

      // Get or create build invocation
      let build = state.builds.get(toolInvocationId);
      if (!build) {
        build = {
          invocation_id: toolInvocationId,
          started_at: timestamp,
          tool_name: toolName,
          tool_version: toolVersion,
          status: 'running',
          targets: new Set<string>(),
          actions: 0,
          correlated_invocations_id: correlatedInvocationsId,
          last_activity: timestamp,
        };
        state.builds.set(toolInvocationId, build);

        // Insert into database
        pool.query(
          `INSERT INTO nativelink_build_invocations (invocation_id, started_at, command, status, flags, meta)
           VALUES ($1, to_timestamp($2 / 1000.0), $3, $4, $5, $6)
           ON CONFLICT (invocation_id) DO NOTHING`,
          [
            toolInvocationId,
            timestamp,
            toolName,
            'running',
            JSON.stringify({ tool_version: toolVersion }),
            JSON.stringify({ correlated_invocations_id: correlatedInvocationsId }),
          ]
        ).catch((err) => {
          console.error('Failed to insert build invocation:', err);
        });
      }

      // Update last activity
      build.last_activity = timestamp;

      // Update build data
      build.actions++;

      // Track targets (only if targetId is not empty)
      if (targetId && targetId.trim()) {
        build.targets.add(targetId);
      }

      // If action_id is "capabilities", this marks the build as started
      if (actionId === 'capabilities') {
        console.log(`🔨 Build started: ${toolName} ${toolVersion} (${toolInvocationId})`);
      }

      // Update database
      pool.query(
        `UPDATE nativelink_build_invocations
         SET meta = jsonb_set(
           COALESCE(meta, '{}'::jsonb),
           '{actions}',
           $1::text::jsonb
         )
         WHERE invocation_id = $2`,
        [build.actions, toolInvocationId]
      ).catch((err) => {
        console.error('Failed to update build actions count:', err);
      });

      // Broadcast build update
      broadcast({
        type: 'builds_updated',
        builds: Array.from(state.builds.values()).map(b => ({
          ...b,
          targets: Array.from(b.targets),
        })),
      });
    }
  }

  // Track build errors - mark builds as failed if we see errors with their invocation ID
  if (level === 'ERROR') {
    // Check for action errors with tool_invocation_id
    const actionErrorMatch = cleanLine.match(LOG_PATTERNS.actionError);
    if (actionErrorMatch) {
      const invocationId = actionErrorMatch[1];
      const build = state.builds.get(invocationId);
      if (build && build.status === 'running') {
        build.status = 'failed';
        build.completed_at = timestamp;

        // Update database
        pool.query(
          `UPDATE nativelink_build_invocations
           SET status = 'failed', completed_at = to_timestamp($1 / 1000.0)
           WHERE invocation_id = $2`,
          [timestamp, invocationId]
        ).catch((err) => {
          console.error('Failed to update build status to failed:', err);
        });

        // Broadcast update
        broadcast({
          type: 'builds_updated',
          builds: Array.from(state.builds.values()).map(b => ({
            ...b,
            targets: Array.from(b.targets),
          })),
        });

        console.log(`❌ Build failed: ${build.tool_name} (${invocationId})`);
      }
    }

    // Check for execution errors with correlated_invocations_id
    const executionErrorMatch = cleanLine.match(LOG_PATTERNS.executionError);
    if (executionErrorMatch) {
      const correlatedId = executionErrorMatch[1];
      // Find builds with this correlated_invocations_id
      for (const [invocationId, build] of state.builds.entries()) {
        if (build.correlated_invocations_id === correlatedId && build.status === 'running') {
          build.status = 'failed';
          build.completed_at = timestamp;

          // Update database
          pool.query(
            `UPDATE nativelink_build_invocations
             SET status = 'failed', completed_at = to_timestamp($1 / 1000.0)
             WHERE invocation_id = $2`,
            [timestamp, invocationId]
          ).catch((err) => {
            console.error('Failed to update build status to failed:', err);
          });

          console.log(`❌ Build failed: ${build.tool_name} (${invocationId})`);
        }
      }

      // Broadcast update
      broadcast({
        type: 'builds_updated',
        builds: Array.from(state.builds.values()).map(b => ({
          ...b,
          targets: Array.from(b.targets),
        })),
      });
    }
  }
}

// Parse log line and extract activity
function parseLogLine(line: string): Activity | null {
  // Remove ANSI color codes
  const cleanLine = line.replace(/\x1b\[[0-9;]*m/g, '');

  // Extract timestamp
  const timestampMatch = cleanLine.match(/(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z)/);
  const timestamp = timestampMatch ? new Date(timestampMatch[1]).getTime() : Date.now();

  // Client connected
  if (LOG_PATTERNS.clientConnected.test(cleanLine)) {
    const match = cleanLine.match(LOG_PATTERNS.clientConnected);
    return {
      uuid: randomUUID(),
      type: 'clients',
      status: 'completed',
      started_at: timestamp,
      updated_at: timestamp,
      meta: {
        'general.from': match?.[1] || 'unknown',
        'event.type': 'client_connected',
      },
    };
  }

  // Worker registered
  if (LOG_PATTERNS.workerRegistered.test(cleanLine)) {
    const match = cleanLine.match(LOG_PATTERNS.workerRegistered);
    const workerId = match?.[1] || '';
    return {
      uuid: randomUUID(),
      type: 'other',
      status: 'completed',
      started_at: timestamp,
      updated_at: timestamp,
      meta: {
        'worker.id': workerId,
        'event.type': 'worker_registered',
      },
    };
  }

  // Worker timeout
  if (LOG_PATTERNS.workerTimeout.test(cleanLine)) {
    const match = cleanLine.match(LOG_PATTERNS.workerTimeout);
    const workerId = match?.[1] || '';
    return {
      uuid: randomUUID(),
      type: 'other',
      status: 'failed',
      started_at: timestamp,
      updated_at: timestamp,
      meta: {
        'worker.id': workerId,
        'event.type': 'worker_timeout',
      },
    };
  }

  // Get capabilities (RBE query)
  if (LOG_PATTERNS.getCapabilities.test(cleanLine)) {
    const match = cleanLine.match(LOG_PATTERNS.getCapabilities);
    return {
      uuid: randomUUID(),
      type: 'rbe',
      status: 'completed',
      started_at: timestamp,
      updated_at: timestamp,
      meta: {
        'rbe.instance_name': match?.[1] || 'main',
        'rbe.command': 'GetCapabilities',
        'event.type': 'get_capabilities',
      },
    };
  }

  // Upload blob
  if (LOG_PATTERNS.uploadBlob.test(cleanLine)) {
    return {
      uuid: randomUUID(),
      type: 'upload',
      status: 'in_progress',
      started_at: timestamp,
      updated_at: timestamp,
      meta: {
        'event.type': 'upload_blob',
      },
    };
  }

  // Download blob
  if (LOG_PATTERNS.downloadBlob.test(cleanLine)) {
    return {
      uuid: randomUUID(),
      type: 'download',
      status: 'in_progress',
      started_at: timestamp,
      updated_at: timestamp,
      meta: {
        'event.type': 'download_blob',
      },
    };
  }

  // Filesystem store - file renamed (cache write)
  if (LOG_PATTERNS.filesystemRenamed.test(cleanLine)) {
    const match = cleanLine.match(LOG_PATTERNS.filesystemRenamed);
    const digest = match?.[1] || '';
    const [hash, size] = digest.split('-');
    return {
      uuid: randomUUID(),
      type: 'upload',
      status: 'completed',
      started_at: timestamp,
      updated_at: timestamp,
      meta: {
        'event.type': 'cache_write',
        'cache.digest': digest,
        'cache.hash': hash,
        'cache.size': size,
        'store.type': 'filesystem',
      },
    };
  }

  // Filesystem store - file deleted (cache eviction)
  if (LOG_PATTERNS.filesystemDeleted.test(cleanLine)) {
    const match = cleanLine.match(LOG_PATTERNS.filesystemDeleted);
    const filePath = match?.[1] || '';
    // Extract digest from path if possible
    const pathMatch = filePath.match(/([a-f0-9]+-\d+)$/);
    const digest = pathMatch?.[1] || '';
    return {
      uuid: randomUUID(),
      type: 'other',
      status: 'completed',
      started_at: timestamp,
      updated_at: timestamp,
      meta: {
        'event.type': 'cache_eviction',
        'cache.digest': digest,
        'cache.path': filePath,
        'store.type': 'filesystem',
      },
    };
  }

  // Filesystem store - spawned delete (background cleanup)
  if (LOG_PATTERNS.filesystemSpawnDelete.test(cleanLine)) {
    const match = cleanLine.match(LOG_PATTERNS.filesystemSpawnDelete);
    const activeSpawns = match?.[1] || '0';
    return {
      uuid: randomUUID(),
      type: 'other',
      status: 'in_progress',
      started_at: timestamp,
      updated_at: timestamp,
      meta: {
        'event.type': 'cache_cleanup',
        'cache.active_spawns': activeSpawns,
        'store.type': 'filesystem',
      },
    };
  }

  return null;
}

// Add activity from log parsing
async function addActivityFromLog(activity: Activity) {
  try {
    // Insert into PostgreSQL
    await pool.query(
      `INSERT INTO nativelink_activities (uuid, type, status, started_at, updated_at, meta)
       VALUES ($1, $2, $3, $4, $5, $6)`,
      [
        activity.uuid,
        activity.type,
        activity.status,
        activity.started_at,
        activity.updated_at,
        JSON.stringify(activity.meta),
      ]
    );

    // Broadcast to connected clients for real-time updates
    broadcast({ type: 'activity_added', activity });
  } catch (error) {
    console.error('Failed to insert activity into database:', error);
  }
}

// Broadcast to all connected clients
function broadcast(message: any) {
  const data = JSON.stringify(message);
  clients.forEach((client) => {
    if (client.readyState === 1) {
      // OPEN
      client.send(data);
    }
  });
}

// WebSocket connection handler
wss.on('connection', async (ws) => {
  console.log('Client connected');
  clients.add(ws);

  try {
    // Query recent activities from database
    const result = await pool.query(
      `SELECT uuid, type, status, started_at, updated_at, meta,
              EXTRACT(EPOCH FROM event_time) * 1000 as event_time
       FROM nativelink_activities
       ORDER BY event_time DESC
       LIMIT 100`
    );

    const activities = result.rows.map((row) => ({
      uuid: row.uuid,
      type: row.type,
      status: row.status,
      started_at: row.started_at,
      updated_at: row.updated_at,
      meta: row.meta,
    }));

    // Send current state
    ws.send(
      JSON.stringify({
        type: 'init',
        workers: Array.from(state.workers.values()),
        clients: Array.from(state.clients.values()),
        logs: state.logs.slice(-100), // Send last 100 logs
        activities: activities,
        builds: Array.from(state.builds.values()).map(b => ({
          ...b,
          targets: Array.from(b.targets),
        })),
      })
    );
  } catch (error) {
    console.error('Failed to fetch activities from database:', error);
    // Send empty state on error
    ws.send(
      JSON.stringify({
        type: 'init',
        workers: Array.from(state.workers.values()),
        clients: Array.from(state.clients.values()),
        logs: state.logs.slice(-100), // Send last 100 logs
        activities: [],
        builds: Array.from(state.builds.values()).map(b => ({
          ...b,
          targets: Array.from(b.targets),
        })),
      })
    );
  }

  ws.on('close', () => {
    console.log('Client disconnected');
    clients.delete(ws);
  });

  ws.on('error', (error) => {
    console.error('WebSocket error:', error);
    clients.delete(ws);
  });
});

// HTTP endpoints
app.get('/health', (req, res) => {
  res.json({ status: 'ok', clients: clients.size });
});

app.get('/api/state', async (req, res) => {
  try {
    // Query recent activities from database
    const result = await pool.query(
      `SELECT uuid, type, status, started_at, updated_at, meta,
              EXTRACT(EPOCH FROM event_time) * 1000 as event_time
       FROM nativelink_activities
       ORDER BY event_time DESC
       LIMIT 100`
    );

    const activities = result.rows.map((row) => ({
      uuid: row.uuid,
      type: row.type,
      status: row.status,
      started_at: row.started_at,
      updated_at: row.updated_at,
      meta: row.meta,
    }));

    res.json({
      workers: Array.from(state.workers.values()),
      clients: Array.from(state.clients.values()),
      logs: state.logs,
      activities: activities,
      builds: Array.from(state.builds.values()).map(b => ({
        ...b,
        targets: Array.from(b.targets),
      })),
      lastUpdate: state.lastUpdate,
    });
  } catch (error) {
    console.error('Failed to fetch activities from database:', error);
    res.status(500).json({
      error: 'Failed to fetch activities',
      workers: Array.from(state.workers.values()),
      clients: Array.from(state.clients.values()),
      logs: state.logs,
      activities: [],
      builds: Array.from(state.builds.values()).map(b => ({
        ...b,
        targets: Array.from(b.targets),
      })),
      lastUpdate: state.lastUpdate,
    });
  }
});

// Parse existing log file to detect workers that registered before backend started
function parseExistingLogFile() {
  const logFile = '/tmp/nativelink-debug.log';

  if (!existsSync(logFile)) {
    console.log('⚠️  Log file does not exist yet, skipping historical parsing');
    return;
  }

  try {
    console.log('📖 Parsing existing log file for historical data...');
    const content = readFileSync(logFile, 'utf8');
    const lines = content.split('\n');
    let workerCount = 0;
    let buildCount = 0;

    for (const line of lines) {
      if (!line.trim()) continue;

      // Remove ANSI color codes
      const cleanLine = line.replace(/\x1b\[[0-9;]*m/g, '');

      // Extract timestamp
      const timestampMatch = cleanLine.match(/(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z)/);
      const timestamp = timestampMatch ? new Date(timestampMatch[1]).getTime() : Date.now();

      // Track worker registrations
      if (LOG_PATTERNS.workerRegistered.test(cleanLine)) {
        const match = cleanLine.match(LOG_PATTERNS.workerRegistered);
        const workerId = match?.[1] || '';

        if (!state.workers.has(workerId)) {
          state.workers.set(workerId, {
            worker_id: workerId,
            status: 'idle',
            last_seen: timestamp,
            platform_properties: {},
          });
          workerCount++;
        }
      }

      // Track build invocations
      if (LOG_PATTERNS.bazelMetadata.test(cleanLine)) {
        const match = cleanLine.match(LOG_PATTERNS.bazelMetadata);
        if (match) {
          const [, toolName, toolVersion, actionId, toolInvocationId, correlatedInvocationsId] = match;

          if (!state.builds.has(toolInvocationId)) {
            state.builds.set(toolInvocationId, {
              invocation_id: toolInvocationId,
              started_at: timestamp,
              tool_name: toolName,
              tool_version: toolVersion,
              status: 'running',
              targets: new Set<string>(),
              actions: 0,
              correlated_invocations_id: correlatedInvocationsId,
              last_activity: timestamp,
            });
            buildCount++;
          } else {
            // Update last activity
            const build = state.builds.get(toolInvocationId);
            if (build) {
              build.last_activity = timestamp;
            }
          }
        }
      }
    }

    console.log(`✅ Parsed ${lines.length} log lines`);
    console.log(`   Found ${workerCount} workers`);
    console.log(`   Found ${buildCount} build invocations`);

    // Broadcast initial state
    if (workerCount > 0) {
      broadcast({
        type: 'workers_updated',
        workers: Array.from(state.workers.values()),
      });
    }
    if (buildCount > 0) {
      broadcast({
        type: 'builds_updated',
        builds: Array.from(state.builds.values()).map(b => ({
          ...b,
          targets: Array.from(b.targets),
        })),
      });
    }
  } catch (error) {
    console.error('❌ Failed to parse existing log file:', error);
  }
}

// Start tailing NativeLink logs via log file
function startLogTailing() {
  const logFile = '/tmp/nativelink-debug.log';

  // Check if log file exists, if not create it
  if (!existsSync(logFile)) {
    console.log(`⚠️  Log file ${logFile} does not exist yet. Will monitor once created.`);
    console.log('💡 Tip: Restart NativeLink with: RUST_LOG=debug ./target/release/nativelink ./toolchain-examples/nativelink-config.json5 2>&1 | tee /tmp/nativelink-debug.log');
    // Try again in 5 seconds
    setTimeout(startLogTailing, 5000);
    return;
  }

  const tail = spawn('tail', ['-f', '-n', '100', logFile]);

  let buffer = '';

  tail.stdout?.on('data', (data) => {
    buffer += data.toString();
    const lines = buffer.split('\n');
    buffer = lines.pop() || ''; // Keep incomplete line in buffer

    for (const line of lines) {
      if (line.trim()) {
        // Process log line for clients, workers, and logs
        processLogLine(line);

        // Parse activity from log line
        const activity = parseLogLine(line);
        if (activity) {
          addActivityFromLog(activity);
          console.log('📝 Parsed activity:', activity.type, activity.meta['event.type']);
        }
      }
    }
  });

  tail.stderr?.on('data', (data) => {
    console.error('Log tail error:', data.toString());
  });

  tail.on('close', (code) => {
    console.log('Log tail process exited:', code);
    // Restart tailing after 5 seconds
    setTimeout(startLogTailing, 5000);
  });

  console.log(`📡 Started tailing NativeLink logs from ${logFile}`);
}

// Check worker timeouts and build completions every 10 seconds
setInterval(() => {
  const now = Date.now();
  const WORKER_TIMEOUT = 60000; // 60 seconds
  const BUILD_INACTIVITY_TIMEOUT = 30000; // 30 seconds - mark builds as succeeded if no activity
  let workersUpdated = false;
  let buildsUpdated = false;

  // Check worker timeouts
  for (const [workerId, worker] of state.workers.entries()) {
    if (worker.status !== 'offline' && now - worker.last_seen > WORKER_TIMEOUT) {
      worker.status = 'offline';
      workersUpdated = true;
    }
  }

  if (workersUpdated) {
    broadcast({
      type: 'workers_updated',
      workers: Array.from(state.workers.values()),
    });
  }

  // Check build timeouts - mark inactive builds as succeeded
  for (const [invocationId, build] of state.builds.entries()) {
    if (build.status === 'running' && now - build.last_activity > BUILD_INACTIVITY_TIMEOUT) {
      build.status = 'succeeded';
      build.completed_at = build.last_activity;
      buildsUpdated = true;

      // Update database
      pool.query(
        `UPDATE nativelink_build_invocations
         SET status = 'succeeded', completed_at = to_timestamp($1 / 1000.0)
         WHERE invocation_id = $2`,
        [build.last_activity, invocationId]
      ).catch((err) => {
        console.error('Failed to update build status to succeeded:', err);
      });

      console.log(`✅ Build completed: ${build.tool_name} (${invocationId}) - ${build.actions} actions`);
    }
  }

  if (buildsUpdated) {
    broadcast({
      type: 'builds_updated',
      builds: Array.from(state.builds.values()).map(b => ({
        ...b,
        targets: Array.from(b.targets),
      })),
    });
  }

  state.lastUpdate = Date.now();
}, 10000);

// API endpoint to query logs from database
app.get('/api/logs', async (req, res) => {
  try {
    const limit = parseInt(req.query.limit as string) || 1000;
    const level = req.query.level as string;
    const offset = parseInt(req.query.offset as string) || 0;

    let query = `
      SELECT
        EXTRACT(EPOCH FROM timestamp) * 1000 as timestamp,
        level,
        message,
        raw
      FROM nativelink_logs
    `;
    const params: any[] = [];

    if (level) {
      query += ` WHERE level = $${params.length + 1}`;
      params.push(level);
    }

    query += ` ORDER BY timestamp DESC LIMIT $${params.length + 1} OFFSET $${params.length + 2}`;
    params.push(limit, offset);

    const result = await pool.query(query, params);

    const logs = result.rows.map((row) => ({
      timestamp: row.timestamp,
      level: row.level,
      message: row.message,
      raw: row.raw,
    }));

    res.json({ logs, count: logs.length });
  } catch (error) {
    console.error('Failed to query logs from database:', error);
    res.status(500).json({ error: 'Failed to query logs', logs: [] });
  }
});

// API endpoint to query build invocations
app.get('/api/builds', async (req, res) => {
  try {
    const limit = parseInt(req.query.limit as string) || 100;
    const offset = parseInt(req.query.offset as string) || 0;
    const status = req.query.status as string;

    let query = `
      SELECT
        invocation_id,
        EXTRACT(EPOCH FROM started_at) * 1000 as started_at,
        EXTRACT(EPOCH FROM completed_at) * 1000 as completed_at,
        command,
        status,
        target,
        flags,
        meta
      FROM nativelink_build_invocations
    `;
    const params: any[] = [];

    if (status) {
      query += ` WHERE status = $${params.length + 1}`;
      params.push(status);
    }

    query += ` ORDER BY started_at DESC LIMIT $${params.length + 1} OFFSET $${params.length + 2}`;
    params.push(limit, offset);

    const result = await pool.query(query, params);

    const builds = result.rows.map((row) => ({
      invocation_id: row.invocation_id,
      started_at: row.started_at,
      completed_at: row.completed_at,
      tool_name: row.command,
      tool_version: row.flags?.tool_version || 'unknown',
      status: row.status || 'running',
      targets: [] as string[], // Will be populated from in-memory state
      actions: row.meta?.actions || 0,
      correlated_invocations_id: row.meta?.correlated_invocations_id || '',
    }));

    // Enrich with in-memory data for targets
    builds.forEach((build) => {
      const memoryBuild = state.builds.get(build.invocation_id);
      if (memoryBuild) {
        build.targets = Array.from(memoryBuild.targets);
        build.actions = memoryBuild.actions;
        build.status = memoryBuild.status;
      }
    });

    res.json({ builds, count: builds.length });
  } catch (error) {
    console.error('Failed to query builds from database:', error);
    res.status(500).json({ error: 'Failed to query builds', builds: [] });
  }
});

// Initialize database and start server
async function startServer() {
  try {
    // Initialize database schema
    await initializeDatabase();

    // Parse existing log file for historical data
    parseExistingLogFile();

    // Start log tailing
    startLogTailing();

    const PORT = 3002;
    server.listen(PORT, () => {
      console.log(`🚀 NativeLink Live backend running on http://localhost:${PORT}`);
      console.log(`📡 WebSocket server ready`);
      console.log(`🔗 Connecting to NativeLink at http://localhost:50061`);
    });
  } catch (error) {
    console.error('Failed to start server:', error);
    process.exit(1);
  }
}

// Start the server
startServer();
