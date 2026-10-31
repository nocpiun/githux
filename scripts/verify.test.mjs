import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import {
  computeSourceIdentity,
  parseRunArgs,
  executeVerifiedCommand,
  acquireBuildSlot,
} from './verify.mjs';

const source = path.dirname(fileURLToPath(import.meta.url));
const verifyScript = path.join(source, 'verify.mjs');

function makeFixtureRoot(t, prefix = 'githux-verify-test-') {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), prefix));
  t.after(() => {
    fs.rmSync(root, { recursive: true, force: true });
  });
  return root;
}

function initGitRepo(dir) {
  const git = args => {
    const r = spawnSync('git', args, { cwd: dir, encoding: 'utf8', windowsHide: true });
    if (r.status !== 0) {
      throw new Error(`git ${args.join(' ')} failed: ${r.stderr}`);
    }
    return r.stdout.trim();
  };
  git(['init']);
  git(['config', 'user.name', 'Test User']);
  git(['config', 'user.email', 'test@example.invalid']);
  git(['config', 'commit.gpgsign', 'false']);
  fs.writeFileSync(path.join(dir, 'README.md'), 'initial content\n');
  git(['add', '.']);
  git(['commit', '-m', 'initial commit']);
  return git;
}

test('computeSourceIdentity captures head and dirty state', t => {
  const root = makeFixtureRoot(t);
  const git = initGitRepo(root);

  // Clean state
  let id = computeSourceIdentity(root);
  assert.match(id.head, /^[a-f0-9]{40}$/);
  assert.strictEqual(id.dirty, false);
  assert.strictEqual(id.changed_count, 0);

  // Dirty state
  fs.writeFileSync(path.join(root, 'README.md'), 'modified content\n');
  id = computeSourceIdentity(root);
  assert.strictEqual(id.dirty, true);
  assert.strictEqual(id.changed_count, 1);
});

test('parseRunArgs parses flags and command correctly', () => {
  const res1 = parseRunArgs(['--timeout', '5000', '--json', '--', 'echo', 'hello world']);
  assert.strictEqual(res1.flags.timeout, 5000);
  assert.strictEqual(res1.flags.json, true);
  assert.strictEqual(res1.command, 'echo');
  assert.deepStrictEqual(res1.args, ['hello world']);

  const res2 = parseRunArgs(['--no-lock', 'node', '-e', 'console.log(1)']);
  assert.strictEqual(res2.flags.noLock, true);
  assert.strictEqual(res2.command, 'node');
  assert.deepStrictEqual(res2.args, ['-e', 'console.log(1)']);

  assert.throws(() => parseRunArgs([]));
  assert.throws(() => parseRunArgs(['--timeout']));
  assert.throws(() => parseRunArgs(['--unknown-flag']));
});

test('executeVerifiedCommand captures stdout, stderr, and exit code', async t => {
  const root = makeFixtureRoot(t);
  initGitRepo(root);

  const nodeCode = `
    console.log('stdout-message');
    console.error('stderr-message');
    process.exit(42);
  `;

  const { record } = await executeVerifiedCommand({
    command: process.execPath,
    args: ['-e', nodeCode],
    worktreeRoot: root,
    noLock: true,
  });

  assert.strictEqual(record.status, 'failed');
  assert.strictEqual(record.exit_code, 42);
  assert.strictEqual(record.signal, null);
  assert.ok(record.run_id);
  assert.ok(record.started_at);
  assert.ok(record.finished_at);
  assert.ok(record.running_duration_ms >= 0);

  const logContent = fs.readFileSync(record.log_path, 'utf8');
  assert.match(logContent, /stdout-message/);
  assert.match(logContent, /stderr-message/);
});

test('executeVerifiedCommand handles successful command', async t => {
  const root = makeFixtureRoot(t);
  initGitRepo(root);

  const { record } = await executeVerifiedCommand({
    command: process.execPath,
    args: ['-e', 'console.log("success");'],
    worktreeRoot: root,
    noLock: true,
  });

  assert.strictEqual(record.status, 'success');
  assert.strictEqual(record.exit_code, 0);
  assert.strictEqual(record.timed_out, false);
  assert.ok(record.log_path.endsWith('.log'));

  const logContent = fs.readFileSync(record.log_path, 'utf8');
  assert.strictEqual(logContent.trim(), 'success');
});

test('executeVerifiedCommand handles paths and args with spaces and unicode', async t => {
  const root = makeFixtureRoot(t);
  initGitRepo(root);

  const specialArgs = ['hello world', 'unicode-测试-🎉', 'quote\'"test'];
  const { record } = await executeVerifiedCommand({
    command: process.execPath,
    args: ['-e', 'console.log(JSON.stringify(process.argv.slice(1)))', ...specialArgs],
    worktreeRoot: root,
    noLock: true,
  });

  assert.strictEqual(record.status, 'success');
  const logContent = fs.readFileSync(record.log_path, 'utf8');
  const parsedArgs = JSON.parse(logContent.trim());
  assert.deepStrictEqual(parsedArgs, specialArgs);
});

test('executeVerifiedCommand handles missing executable', async t => {
  const root = makeFixtureRoot(t);
  initGitRepo(root);

  const { record } = await executeVerifiedCommand({
    command: 'nonexistent-executable-githux-test',
    args: ['arg1'],
    worktreeRoot: root,
    noLock: true,
  });

  assert.strictEqual(record.status, 'failed');
  assert.strictEqual(record.exit_code, null);
  assert.ok(record.spawn_error);
  assert.match(record.spawn_error, /ENOENT/);
});

test('executeVerifiedCommand handles timeout and preserves record', async t => {
  const root = makeFixtureRoot(t);
  initGitRepo(root);

  const startTime = Date.now();
  const { record } = await executeVerifiedCommand({
    command: process.execPath,
    args: ['-e', 'setTimeout(() => {}, 60000);'],
    worktreeRoot: root,
    noLock: true,
    timeoutMs: 1500,
  });
  const wallTime = Date.now() - startTime;

  assert.ok(record.timed_out);
  assert.strictEqual(record.status, 'timed_out');
  assert.notStrictEqual(record.exit_code, 0);
  assert.ok(wallTime >= 1000 && wallTime < 10000);

  // Record must be written to disk and not be green
  const savedRecord = JSON.parse(fs.readFileSync(record.log_path.replace('.log', '.json'), 'utf8'));
  assert.strictEqual(savedRecord.status, 'timed_out');
  assert.ok(savedRecord.timed_out);
});

test('executeVerifiedCommand serializes runs with Windows mutex using harmless fixture children', async t => {
  if (process.platform !== 'win32') {
    t.skip('Windows mutex test only applies on Windows');
    return;
  }

  const root = makeFixtureRoot(t);
  initGitRepo(root);

  const cmd = process.execPath;
  const nodeScript = `setTimeout(() => { console.log('done'); }, 300);`;

  const run1 = executeVerifiedCommand({
    command: cmd,
    args: ['-e', nodeScript],
    worktreeRoot: root,
    noLock: false, // use the real Windows mutex
  });

  const run2 = executeVerifiedCommand({
    command: cmd,
    args: ['-e', nodeScript],
    worktreeRoot: root,
    noLock: false,
  });

  const [r1, r2] = await Promise.all([run1, run2]);

  assert.strictEqual(r1.record.status, 'success');
  assert.strictEqual(r2.record.status, 'success');

  // At least one of the two should have waited for the mutex
  const hasWait = r1.record.waiting_for_slot_ms > 0 || r2.record.waiting_for_slot_ms > 0;
  assert.ok(hasWait, 'Second invocation should have waited for the shared mutex');
}, { timeout: 30000 });

test('CLI preserves child exit code', async t => {
  const root = makeFixtureRoot(t);
  initGitRepo(root);

  const result = await new Promise(resolve => {
    const child = spawn(process.execPath, [verifyScript, '--no-lock', '--', process.execPath, '-e', 'process.exit(9)'], {
      cwd: root,
      stdio: 'pipe',
      windowsHide: true,
    });
    let output = '';
    child.stdout.on('data', d => output += d.toString());
    child.stderr.on('data', d => output += d.toString());
    child.on('close', code => resolve({ code, output }));
  });

  assert.strictEqual(result.code, 9);
  assert.match(result.output, /FAILED \(exit 9/);
});

test('Real Cargo invocation: failed cargo check preserves non-zero exit', async t => {
  const root = makeFixtureRoot(t);
  initGitRepo(root);

  const { record } = await executeVerifiedCommand({
    command: 'cargo',
    args: ['check', '--lib', '--nonexistent-cargo-flag'],
    worktreeRoot: root,
    noLock: true,
  });

  assert.strictEqual(record.status, 'failed');
  assert.ok(record.exit_code !== 0 && record.exit_code !== null);
  const logContent = fs.readFileSync(record.log_path, 'utf8');
  assert.match(logContent, /unexpected argument|error:/i);
});

test('Real Cargo invocation: small successful check yields truthful record', async t => {
  const root = makeFixtureRoot(t);
  initGitRepo(root);

  const { record } = await executeVerifiedCommand({
    command: 'cargo',
    args: ['--version'],
    worktreeRoot: root,
    noLock: true,
  });

  assert.strictEqual(record.status, 'success');
  assert.strictEqual(record.exit_code, 0);
  const logContent = fs.readFileSync(record.log_path, 'utf8');
  assert.match(logContent, /cargo \d+\.\d+\.\d+/i);
});
