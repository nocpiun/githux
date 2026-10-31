import { spawn, spawnSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const SELF = fileURLToPath(import.meta.url);
const MUTEX_NAME = 'Local\\Githux-Cargo-20261029';

function fail(msg) {
  throw new Error(msg);
}

function nowIso() {
  return new Date().toISOString();
}

function encodePS(source) {
  return Buffer.from(source, 'utf16le').toString('base64');
}

function powershellPath() {
  return path.join(
    process.env.SystemRoot ?? 'C:/Windows',
    'System32',
    'WindowsPowerShell',
    'v1.0',
    'powershell.exe'
  );
}

function getWorktreeRoot(startDir = process.cwd()) {
  const res = spawnSync('git', ['rev-parse', '--show-toplevel'], {
    cwd: startDir,
    encoding: 'utf8',
    windowsHide: true,
  });
  if (res.status === 0 && res.stdout.trim()) {
    return path.resolve(res.stdout.trim());
  }
  return path.resolve(startDir);
}

function getGitHeadSha(worktreeRoot) {
  const res = spawnSync('git', ['rev-parse', 'HEAD'], {
    cwd: worktreeRoot,
    encoding: 'utf8',
    windowsHide: true,
  });
  if (res.status === 0 && res.stdout.trim()) {
    return res.stdout.trim();
  }
  return 'unknown';
}

function getGitStatusSummary(worktreeRoot) {
  const res = spawnSync('git', ['status', '--porcelain=v1', '-uall'], {
    cwd: worktreeRoot,
    encoding: 'utf8',
    windowsHide: true,
  });
  if (res.status !== 0) {
    return { is_dirty: null, error: res.stderr.trim() };
  }
  const lines = res.stdout.split(/\r?\n/).filter(Boolean);
  return {
    is_dirty: lines.length > 0,
    changed_count: lines.length,
    summary: lines.slice(0, 20),
  };
}

export function computeSourceIdentity(worktreeRoot) {
  const head = getGitHeadSha(worktreeRoot);
  const status = getGitStatusSummary(worktreeRoot);
  return {
    head,
    dirty: status.is_dirty,
    changed_count: status.changed_count ?? 0,
    status_sample: status.summary ?? [],
  };
}

function atomicWriteJson(targetPath, data) {
  const dir = path.dirname(targetPath);
  if (!fs.existsSync(dir)) {
    fs.mkdirSync(dir, { recursive: true });
  }
  const tmp = `${targetPath}.${randomUUID()}.tmp`;
  fs.writeFileSync(tmp, JSON.stringify(data, null, 2) + '\n', 'utf8');
  fs.renameSync(tmp, targetPath);
}

export function acquireBuildSlotWindows({
  mutexName = MUTEX_NAME,
  pollIntervalMs = 5000,
  maxWaitMs = null,
  onWaiting = null,
} = {}) {
  // We use a small PowerShell helper process to hold the Win32 Named Mutex.
  // Communication with helper:
  // stdin: send 'QUIT\n' to release and exit
  // stdout lines:
  //   'ACQUIRED\n'
  //   'WAITING\n'
  //   'ERROR: ...\n'
  // stderr: diagnostic errors
  const psCode = `
$ErrorActionPreference = 'Stop'
$mutexName = '${mutexName}'
$guard = $null
try {
    $guard = [System.Threading.Mutex]::new($false, $mutexName)
} catch {
    [Console]::Out.WriteLine('ERROR: ' + $_.Exception.Message)
    [Console]::Out.Flush()
    exit 2
}

$acquired = $false
while (-not $acquired) {
    try {
        $acquired = $guard.WaitOne(${pollIntervalMs})
    } catch [System.Threading.AbandonedMutexException] {
        $acquired = $true
    } catch {
        [Console]::Out.WriteLine('ERROR: ' + $_.Exception.Message)
        [Console]::Out.Flush()
        if ($guard) { $guard.Dispose() }
        exit 3
    }
    if ($acquired) {
        [Console]::Out.WriteLine('ACQUIRED')
        [Console]::Out.Flush()
    } else {
        [Console]::Out.WriteLine('WAITING')
        [Console]::Out.Flush()
    }
}

try {
    $reader = [System.Console]::In
    while ($true) {
        $line = $reader.ReadLine()
        if ($line -eq $null -or $line -eq 'QUIT') { break }
    }
} finally {
    if ($acquired -and $guard) {
        try { $guard.ReleaseMutex() } catch {}
    }
    if ($guard) { $guard.Dispose() }
}
exit 0
`;

  return new Promise((resolve, reject) => {
    const ps = spawn(
      powershellPath(),
      ['-NoProfile', '-NonInteractive', '-EncodedCommand', encodePS(psCode)],
      {
        stdio: ['pipe', 'pipe', 'pipe'],
        windowsHide: true,
      }
    );

    let acquired = false;
    let stdoutBuffer = '';
    const waitStartTime = Date.now();
    let waitTimer = null;

    if (maxWaitMs !== null && maxWaitMs > 0) {
      waitTimer = setTimeout(() => {
        cleanup();
        reject(new Error(`Timeout waiting for build slot after ${maxWaitMs}ms`));
      }, maxWaitMs);
    }

    function cleanup() {
      if (waitTimer) {
        clearTimeout(waitTimer);
        waitTimer = null;
      }
      try {
        ps.stdin.write('QUIT\n');
        ps.stdin.end();
      } catch {}
    }

    ps.stdout.on('data', chunk => {
      stdoutBuffer += chunk.toString('utf8');
      const lines = stdoutBuffer.split(/\r?\n/);
      stdoutBuffer = lines.pop(); // keep last incomplete line

      for (const line of lines) {
        const trimmed = line.trim();
        if (trimmed === 'ACQUIRED') {
          acquired = true;
          if (waitTimer) {
            clearTimeout(waitTimer);
            waitTimer = null;
          }
          const waitDurationMs = Date.now() - waitStartTime;
          resolve({
            waitDurationMs,
            release: () => {
              return new Promise(relResolve => {
                ps.on('exit', () => relResolve());
                cleanup();
              });
            },
          });
          return;
        } else if (trimmed === 'WAITING') {
          if (onWaiting) {
            onWaiting({ waitElapsedMs: Date.now() - waitStartTime });
          }
        } else if (trimmed.startsWith('ERROR:')) {
          cleanup();
          reject(new Error(trimmed));
          return;
        }
      }
    });

    ps.stderr.on('data', chunk => {
      // Diagnostic output from powershell if any
    });

    ps.on('error', err => {
      cleanup();
      reject(err);
    });

    ps.on('exit', (code, signal) => {
      if (!acquired) {
        cleanup();
        reject(new Error(`Slot helper exited prematurely with code ${code}`));
      }
    });
  });
}

export async function acquireBuildSlot({
  platform = process.platform,
  mutexName = MUTEX_NAME,
  pollIntervalMs = 5000,
  maxWaitMs = null,
  onWaiting = null,
} = {}) {
  if (platform === 'win32') {
    return acquireBuildSlotWindows({
      mutexName,
      pollIntervalMs,
      maxWaitMs,
      onWaiting,
    });
  }
  // On non-Windows platforms (e.g. Linux/macOS in CI), no Windows mutex exists.
  return {
    waitDurationMs: 0,
    release: async () => {},
  };
}

export function parseRunArgs(rawArgs) {
  // Usage:
  // verify.mjs [runner-flags] -- command [args...]
  // Flags:
  //   --timeout <ms>
  //   --task-dir <dir>
  //   --no-lock
  //   --json
  const flags = {
    timeout: 0,
    taskDir: null,
    noLock: false,
    json: false,
  };
  const commandArgs = [];
  let foundSeparator = false;

  for (let i = 0; i < rawArgs.length; i++) {
    const arg = rawArgs[i];
    if (foundSeparator) {
      commandArgs.push(arg);
      continue;
    }
    if (arg === '--') {
      foundSeparator = true;
      continue;
    }
    if (arg === '--timeout') {
      i++;
      if (i >= rawArgs.length) fail('Missing value for --timeout');
      flags.timeout = parseInt(rawArgs[i], 10);
      if (isNaN(flags.timeout) || flags.timeout < 0) fail('Invalid --timeout value');
    } else if (arg === '--task-dir') {
      i++;
      if (i >= rawArgs.length) fail('Missing value for --task-dir');
      flags.taskDir = rawArgs[i];
    } else if (arg === '--no-lock') {
      flags.noLock = true;
    } else if (arg === '--json') {
      flags.json = true;
    } else if (arg.startsWith('-')) {
      fail(`Unknown runner option: ${arg}`);
    } else {
      // First non-flag without '--' separator starts the command
      commandArgs.push(...rawArgs.slice(i));
      break;
    }
  }

  if (commandArgs.length === 0) {
    fail('No command specified to run. Usage: verify.mjs [--flags] -- <executable> [args...]');
  }

  return { flags, command: commandArgs[0], args: commandArgs.slice(1) };
}

export async function executeVerifiedCommand({
  command,
  args = [],
  worktreeRoot = null,
  taskDir = null,
  timeoutMs = 0,
  noLock = false,
  platform = process.platform,
  mutexName = MUTEX_NAME,
  env = process.env,
  onWaiting = null,
} = {}) {
  const root = worktreeRoot ? path.resolve(worktreeRoot) : getWorktreeRoot();
  const taskStateDir = taskDir ? path.resolve(taskDir) : path.join(root, '.agents', 'task');
  const runsDir = path.join(taskStateDir, 'runs');

  const runId = randomUUID();
  const logFile = path.join(runsDir, `${runId}.log`);
  const recordFile = path.join(runsDir, `${runId}.json`);

  if (!fs.existsSync(runsDir)) {
    fs.mkdirSync(runsDir, { recursive: true });
  }

  const sourceIdentity = computeSourceIdentity(root);
  const startTime = Date.now();
  const startedAtIso = nowIso();

  const record = {
    run_id: runId,
    command,
    args,
    worktree: root,
    source_identity: sourceIdentity,
    started_at: startedAtIso,
    finished_at: null,
    status: 'running',
    waiting_for_slot_ms: 0,
    running_duration_ms: 0,
    wall_duration_ms: 0,
    exit_code: null,
    signal: null,
    spawn_error: null,
    timed_out: false,
    child_pid: null,
    log_path: logFile,
  };

  atomicWriteJson(recordFile, record);

  let slot = null;
  if (!noLock) {
    try {
      slot = await acquireBuildSlot({
        platform,
        mutexName,
        pollIntervalMs: 5000,
        onWaiting,
      });
      record.waiting_for_slot_ms = slot.waitDurationMs;
    } catch (err) {
      record.status = 'failed';
      record.spawn_error = `Slot acquisition failed: ${err.message}`;
      record.finished_at = nowIso();
      record.wall_duration_ms = Date.now() - startTime;
      atomicWriteJson(recordFile, record);
      throw err;
    }
  }

  const logStream = fs.createWriteStream(logFile, { flags: 'wx', encoding: 'utf8' });
  const runStartTime = Date.now();

  return new Promise((resolve) => {
    let child = null;
    let timedOut = false;
    let timeoutTimer = null;
    let ended = false;

    // Build environment overrides: enforce 1 cargo job on Windows to respect slot constraints
    const childEnv = { ...env };
    if (command.toLowerCase().includes('cargo')) {
      childEnv.CARGO_BUILD_JOBS = '1';
    }

    try {
      child = spawn(command, args, {
        cwd: root,
        env: childEnv,
        stdio: ['ignore', 'pipe', 'pipe'],
        windowsHide: true,
      });
    } catch (err) {
      if (slot) {
        slot.release().catch(() => {});
      }
      logStream.write(`Failed to spawn child: ${err.message}\n`);
      logStream.end();
      record.status = 'failed';
      record.spawn_error = err.message;
      record.finished_at = nowIso();
      record.running_duration_ms = Date.now() - runStartTime;
      record.wall_duration_ms = Date.now() - startTime;
      atomicWriteJson(recordFile, record);
      resolve({ record, logContent: `Failed to spawn: ${err.message}` });
      return;
    }

    record.child_pid = child.pid ?? null;
    atomicWriteJson(recordFile, record);

    const signalHandler = (signal) => {
      if (ended) return;
      ended = true;
      if (timeoutTimer) clearTimeout(timeoutTimer);
      process.removeListener('SIGINT', signalHandler);
      process.removeListener('SIGTERM', signalHandler);
      logStream.write(`\n[Runner] Received ${signal}. Terminating child and waiting for confirmation...\n`);
      if (child && child.pid) {
        try {
          if (process.platform === 'win32') {
            spawnSync('taskkill', ['/pid', String(child.pid), '/T', '/F'], { windowsHide: true });
          } else {
            child.kill('SIGKILL');
          }
        } catch {}
      }
      logStream.end(async () => {
        if (slot) {
          await slot.release().catch(() => {});
        }
        record.status = 'cancelled';
        record.signal = signal;
        record.finished_at = nowIso();
        record.running_duration_ms = Date.now() - runStartTime;
        record.wall_duration_ms = Date.now() - startTime;
        atomicWriteJson(recordFile, record);
        resolve({ record });
      });
    };

    process.on('SIGINT', signalHandler);
    process.on('SIGTERM', signalHandler);

    if (timeoutMs > 0) {
      timeoutTimer = setTimeout(() => {
        timedOut = true;
        logStream.write(`\n[Runner] Command timed out after ${timeoutMs}ms. Terminating child...\n`);
        if (child && !ended) {
          try {
            if (process.platform === 'win32' && child.pid) {
              spawnSync('taskkill', ['/pid', String(child.pid), '/T', '/F'], { windowsHide: true });
            } else {
              child.kill('SIGTERM');
              // Escalate to SIGKILL after a grace period if SIGTERM is ignored
              const killTimer = setTimeout(() => {
                if (child && !ended) {
                  try { child.kill('SIGKILL'); } catch {}
                }
              }, 3000);
              killTimer.unref();
            }
          } catch {}
        }
      }, timeoutMs);
    }

    child.stdout.on('data', chunk => {
      try {
        logStream.write(chunk);
      } catch {}
    });

    child.stderr.on('data', chunk => {
      try {
        logStream.write(chunk);
      } catch {}
    });

    child.on('error', err => {
      if (ended) return;
      ended = true;
      if (timeoutTimer) clearTimeout(timeoutTimer);
      process.removeListener('SIGINT', signalHandler);
      process.removeListener('SIGTERM', signalHandler);
      logStream.write(`\nChild error: ${err.message}\n`);
      logStream.end(async () => {
        if (slot) {
          await slot.release().catch(() => {});
        }
        record.status = 'failed';
        record.spawn_error = err.message;
        record.finished_at = nowIso();
        record.running_duration_ms = Date.now() - runStartTime;
        record.wall_duration_ms = Date.now() - startTime;
        atomicWriteJson(recordFile, record);
        resolve({ record });
      });
    });

    child.on('close', (code, signal) => {
      if (ended) return;
      ended = true;
      if (timeoutTimer) clearTimeout(timeoutTimer);
      process.removeListener('SIGINT', signalHandler);
      process.removeListener('SIGTERM', signalHandler);
      logStream.end(async () => {
        if (slot) {
          await slot.release().catch(() => {});
        }
        record.finished_at = nowIso();
        record.running_duration_ms = Date.now() - runStartTime;
        record.wall_duration_ms = Date.now() - startTime;
        record.exit_code = code;
        record.signal = signal;
        record.timed_out = timedOut;

        if (timedOut) {
          record.status = 'timed_out';
        } else if (code === 0) {
          record.status = 'success';
        } else {
          record.status = 'failed';
        }

        atomicWriteJson(recordFile, record);
        resolve({ record });
      });
    });
  });
}

// CLI Entry point
async function main() {
  const rawArgs = process.argv.slice(2);
  const sepIndex = rawArgs.indexOf('--');
  const runnerArgs = sepIndex >= 0 ? rawArgs.slice(0, sepIndex) : rawArgs;
  if (rawArgs.length === 0 || runnerArgs.includes('--help') || runnerArgs.includes('-h')) {
    console.log(`Githux Verification Runner
Usage:
  node scripts/verify.mjs [options] -- <executable> [args...]

Options:
  --timeout <ms>    Execution timeout in milliseconds
  --task-dir <dir>  Directory to store .agents/task/runs/ state
  --no-lock         Do not acquire the shared Rust build slot mutex
  --json            Output complete run record JSON
  --help, -h        Show this help
`);
    return;
  }

  const { flags, command, args } = parseRunArgs(rawArgs);

  const onWaiting = ({ waitElapsedMs }) => {
    process.stderr.write(`Waiting for shared Rust build slot (${Math.round(waitElapsedMs / 1000)}s elapsed)...\n`);
  };

  const { record } = await executeVerifiedCommand({
    command,
    args,
    taskDir: flags.taskDir,
    timeoutMs: flags.timeout,
    noLock: flags.noLock,
    onWaiting,
  });

  if (flags.json) {
    console.log(JSON.stringify(record, null, 2));
  } else {
    const outcome = record.status === 'success' ? 'SUCCESS' : (record.timed_out ? 'TIMED OUT' : 'FAILED');
    console.log(`Verification run ${record.run_id}: ${outcome} (exit ${record.exit_code ?? 'none'}, signal: ${record.signal ?? 'none'})`);
    console.log(`Command: ${record.command} ${record.args.join(' ')}`);
    console.log(`Duration: running ${record.running_duration_ms}ms, slot wait ${record.waiting_for_slot_ms}ms, wall ${record.wall_duration_ms}ms`);
    console.log(`Log path: ${record.log_path}`);
  }

  if (record.status !== 'success' || record.exit_code !== 0) {
    process.exit(record.exit_code ?? 1);
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === path.resolve(SELF)) {
  main().catch(err => {
    console.error(`Verification runner error: ${err.message}`);
    process.exit(1);
  });
}
