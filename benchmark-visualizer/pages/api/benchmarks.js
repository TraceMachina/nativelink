import fs from 'fs';
import path from 'path';

const DEFAULT_RESULTS_DIR = '/Users/diouf/nativelink/tools/benchmark/benchmark_results';
const ENV_RESULTS_DIR = process.env.BENCHMARK_RESULTS_DIR;

export default function handler(req, res) {
  try {
    const resultsDir = ENV_RESULTS_DIR || DEFAULT_RESULTS_DIR;
    
    // Enhanced permission check with detailed error reporting
    const checkPermissions = () => {
      try {
        // Check if directory exists
        if (!fs.existsSync(resultsDir)) {
          try {
            fs.mkdirSync(resultsDir, { 
              recursive: true,
              mode: 0o755  // rwxr-xr-x permissions
            });
          } catch (mkdirError) {
            const errorDetails = {
              error: mkdirError,
              stack: mkdirError.stack,
              path: resultsDir,
              currentUser: process.env.USER || process.env.USERNAME,
              processInfo: {
                uid: process.getuid(),
                gid: process.getgid(),
                cwd: process.cwd()
              }
            };
            console.error('Error creating benchmark directory:', errorDetails);
            return {
              status: 500,
              error: 'directory_creation_failed',
              message: 'Failed to create benchmark results directory',
              details: mkdirError.message,
              debugInfo: errorDetails,
              userFriendlyMessage: 'The benchmark data directory could not be created. Please check if the path is correct and you have sufficient permissions.'
            };
          }
        }

        // Verify directory permissions
        try {
          fs.accessSync(resultsDir, fs.constants.R_OK | fs.constants.W_OK);
          return null; // No error
        } catch (accessError) {
          const stats = fs.existsSync(resultsDir) ? fs.statSync(resultsDir) : null;
          const errorDetails = {
            error: accessError,
            stack: accessError.stack,
            path: resultsDir,
            currentUser: process.env.USER || process.env.USERNAME,
            directoryStats: stats ? {
              mode: stats.mode.toString(8),
              uid: stats.uid,
              gid: stats.gid,
              isDirectory: stats.isDirectory()
            } : null,
            processInfo: {
              uid: process.getuid(),
              gid: process.getgid(),
              cwd: process.cwd()
            }
          };
          console.error('Permission error accessing benchmark directory:', errorDetails);
          return {
            status: 403,
            error: 'permission_denied',
            message: 'Insufficient permissions to access benchmark data',
            details: `Current user ${process.env.USER || process.env.USERNAME} lacks read/write access to ${resultsDir}`,
            debugInfo: errorDetails,
            remediationSteps: [
              'Check directory ownership and permissions',
              'Verify the Node.js process has sufficient privileges',
              `Run: chmod 755 ${resultsDir} to fix permissions`,
              `Run: chown ${process.env.USER || process.env.USERNAME} ${resultsDir} to fix ownership`
            ],
            userFriendlyMessage: 'You don\'t have permission to access the benchmark data. Please contact your system administrator or check the directory permissions.'
          };
        }
      } catch (error) {
        console.error('Unexpected error checking permissions:', error);
        return {
          status: 500,
          error: 'unexpected_error',
          message: 'Unexpected error checking permissions',
          details: error.message
        };
      }
    };

    const permissionError = checkPermissions();
    if (permissionError) {
      // Log detailed error information for debugging
      console.error('Permission error details:', {
        timestamp: new Date().toISOString(),
        user: process.env.USER || process.env.USERNAME,
        directory: resultsDir,
        errorDetails: permissionError
      });
      
      return res.status(permissionError.status).json({
        error: permissionError.error,
        message: permissionError.message,
        details: permissionError.details,
        ...(permissionError.debugInfo && { debugInfo: permissionError.debugInfo }),
        ...(permissionError.remediationSteps && { remediationSteps: permissionError.remediationSteps }),
        userFriendlyMessage: permissionError.userFriendlyMessage || 'Unable to access benchmark data due to permission issues.',
        code: 403,
        handled: false,
        httpStatus: permissionError.status,
        httpStatusText: '',
        originalError: {
          name: 'Error',
          message: permissionError.message,
          stack: permissionError.debugInfo?.error?.stack || ''
        }
      });
    }
    
    try {
      // Check directory access with more detailed error handling
      try {
        fs.accessSync(resultsDir, fs.constants.R_OK | fs.constants.W_OK);
      } catch (accessError) {
        console.error('Permission error accessing benchmark directory:', {
          error: accessError,
          stack: accessError.stack,
          path: resultsDir,
          currentUser: process.env.USER || process.env.USERNAME,
          directoryStats: fs.existsSync(resultsDir) ? {
            mode: fs.statSync(resultsDir).mode.toString(8),
            uid: fs.statSync(resultsDir).uid,
            gid: fs.statSync(resultsDir).gid
          } : null
        });
        return res.status(403).json({
          error: 'permission_denied',
          message: 'Insufficient permissions to access benchmark data',
          details: `Current user ${process.env.USER || process.env.USERNAME} lacks read/write access to ${resultsDir}`,
          userFriendlyMessage: 'You don\'t have permission to access the benchmark data. Please contact your system administrator or check the directory permissions.',
          code: 403,
          handled: false,
          httpStatus: 403,
          httpStatusText: '',
          originalError: {
            name: 'Error',
            message: 'permission error',
            stack: ''
          },
          reqInfo: {
            pathPrefix: req.url,
            method: req.method,
            path: req.url
          }
        });
      }
      
      // Log directory stats for debugging
      const stats = fs.statSync(resultsDir);
      console.log('Benchmark directory stats:', {
        mode: stats.mode.toString(8),
        uid: stats.uid,
        gid: stats.gid,
        path: resultsDir,
        permissions: {
          readable: fs.accessSync(resultsDir, fs.constants.R_OK),
          writable: fs.accessSync(resultsDir, fs.constants.W_OK),
          executable: fs.accessSync(resultsDir, fs.constants.X_OK)
        }
      });
      
      // Verify directory is actually a directory
      if (!stats.isDirectory()) {
        throw new Error(`Path ${resultsDir} exists but is not a directory`);
      }
      
      // Check file permissions in the directory
      const testFile = path.join(resultsDir, '.permission_test');
      try {
        fs.writeFileSync(testFile, 'test', { mode: 0o644 });
        fs.unlinkSync(testFile);
      } catch (fileError) {
        throw new Error(`Cannot write to directory ${resultsDir}: ${fileError.message}`);
      }
      
      const files = fs.readdirSync(resultsDir).filter(file => file.endsWith('.json'));
      
      if (files.length === 0) {
        return res.status(200).json({
          status: 'success',
          message: 'No benchmark data found',
          data: [],
          metadata: {
            directory: resultsDir,
            timestamp: new Date().toISOString(),
            count: 0
          }
        });
      }
      
      const benchmarks = [];
      
      files.forEach(file => {
        try {
          const filePath = path.join(resultsDir, file);
          const fileContent = fs.readFileSync(filePath, 'utf8');
          const data = JSON.parse(fileContent);
          if (data) {
            benchmarks.push({
              ...data,
              build_remote_execution_time_sec: data.metrics?.remote_execution?.build_time || 0
            });
          }
        } catch (fileError) {
          console.error('Error accessing/parsing benchmark file:', fileError);
          // Continue to next file instead of failing the entire request
        }
      });
      
      res.status(200).json(benchmarks || []);
    } catch (accessError) {
      const directoryStats = fs.existsSync(resultsDir) ? fs.statSync(resultsDir) : null;
      
      console.error('Permission error accessing benchmark data:', {
        error: accessError,
        stack: accessError.stack,
        path: resultsDir,
        currentUser: process.env.USER || process.env.USERNAME,
        directoryStats: directoryStats ? {
          mode: directoryStats.mode.toString(8),
          uid: directoryStats.uid,
          gid: directoryStats.gid,
          isDirectory: directoryStats.isDirectory()
        } : null,
        processInfo: {
          uid: process.getuid(),
          gid: process.getgid(),
          cwd: process.cwd()
        },
        effectivePermissions: {
          readable: fs.existsSync(resultsDir) ? fs.accessSync(resultsDir, fs.constants.R_OK) : false,
          writable: fs.existsSync(resultsDir) ? fs.accessSync(resultsDir, fs.constants.W_OK) : false,
          executable: fs.existsSync(resultsDir) ? fs.accessSync(resultsDir, fs.constants.X_OK) : false
        },
        environment: {
          NODE_ENV: process.env.NODE_ENV,
          PWD: process.env.PWD
        }
      });
      
      res.status(403).json({ 
        error: 'permission_error', 
        message: 'Insufficient permissions to access benchmark data',
        path: resultsDir,
        details: accessError.message,
        requiredPermissions: 'read and write access',
        suggestion: 'Verify the application has proper permissions to access this directory',
        additionalInfo: {
          directoryExists: fs.existsSync(resultsDir),
          directoryOwner: directoryStats?.uid,
          directoryGroup: directoryStats?.gid,
          directoryMode: directoryStats?.mode.toString(8),
          processUser: process.getuid(),
          processGroup: process.getgid(),
          readable: fs.existsSync(resultsDir) ? fs.accessSync(resultsDir, fs.constants.R_OK) : false,
          writable: fs.existsSync(resultsDir) ? fs.accessSync(resultsDir, fs.constants.W_OK) : false,
          executable: fs.existsSync(resultsDir) ? fs.accessSync(resultsDir, fs.constants.X_OK) : false,
          environment: {
            NODE_ENV: process.env.NODE_ENV,
            PWD: process.env.PWD,
            USER: process.env.USER || process.env.USERNAME
          }
        },
        remediationSteps: [
          'Check directory ownership and permissions',
          'Verify the Node.js process has sufficient privileges',
          'Ensure the directory exists at the specified path',
          'Check directory mode (current: ' + (directoryStats?.mode.toString(8) || 'unknown') + ')',
          'Verify executable bit is set if needed',
          'Try running: chmod -R 755 ' + resultsDir,
          'Try running: chown -R ' + (process.env.USER || process.env.USERNAME) + ' ' + resultsDir
        ],
        userFriendlyMessage: 'Permission denied while accessing benchmark data. Please ensure the application has read/write access to the directory.',
        httpError: false,
        httpStatus: 403,
        httpStatusText: '',
        code: 403,
        handled: false,
        originalError: {
          name: 'Error',
          message: accessError.message,
          stack: accessError.stack
        },
        reqInfo: {
          pathPrefix: req.url,
          method: req.method,
          path: req.url
        }
      });
    }
  } catch (error) {
    console.error('Error reading benchmark data:', error);
    res.status(500).json({ 
      error: 'server_error', 
      message: 'Failed to load benchmark data' 
    });
  }
}