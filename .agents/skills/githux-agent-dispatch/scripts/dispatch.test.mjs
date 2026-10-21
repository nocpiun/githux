import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn, spawnSync } from 'node:child_process';
import { createTask, launchTask, taskStatus, loadConfig, processTable, encodePS, terminalCommand } from './dispatch.mjs';

const source = path.dirname(fileURLToPath(import.meta.url));
const script = path.join(source, 'dispatch.mjs');
const git = (repo, args) => {
  const r = spawnSync('git', ['-C', repo, ...args], { encoding: 'utf8', windowsHide: true });
  assert.equal(r.status, 0, r.stderr);
  return r.stdout.trim();
};
const json = p => JSON.parse(fs.readFileSync(p, 'utf8'));
const put = (p, value) => fs.writeFileSync(p, JSON.stringify(value));
async function until(fn, limit = 30000) {
  const deadline = Date.now() + limit;
  while (Date.now() < deadline) { const value = fn(); if (value) return value; await new Promise(r => setTimeout(r, 100)); }
  throw new Error('Timed out waiting for fixture process');
}
function fixture(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'githux-dispatch-test-'));
  const repo = path.join(root, 'repo 空格 & % !');
  const parent = path.join(root, 'worktrees 空格 & % !');
  const cliDir = path.join(root, 'executor 空格 & % !');
  for (const d of [repo, parent, cliDir]) fs.mkdirSync(d);
  git(repo, ['init']);
  fs.mkdirSync(path.join(repo, 'docs'));
  fs.writeFileSync(path.join(repo, 'docs', 'README.md'), 'committed shared facts');
  fs.writeFileSync(path.join(repo, 'AGENTS.md'), 'shared agents');
  fs.writeFileSync(path.join(repo, '.gitignore'), '.agents/*\n!.agents/skills/\n');
  git(repo, ['add', '.']);
  git(repo, ['-c', 'user.name=Dispatch Test', '-c', 'user.email=dispatch-test@example.invalid', '-c', 'commit.gpgsign=false', 'commit', '-m', 'fixture base']);
  const base = git(repo, ['rev-parse', 'HEAD']);
  fs.writeFileSync(path.join(repo, 'docs', 'README.md'), 'uncommitted facts must not copy');
  fs.mkdirSync(path.join(repo, '.agents'));
  fs.writeFileSync(path.join(repo, '.agents', 'secret.json'), 'private fixture secret');
  fs.writeFileSync(path.join(repo, 'untracked-private.txt'), 'private untracked fixture');
  const cli = path.join(cliDir, 'fake-cli.mjs');
  fs.copyFileSync(path.join(source, 'fixtures', 'fake-cli.mjs'), cli);
  const settings = path.join(cliDir, 'fake-settings.json');
  put(settings, { provider: 'fixture', model: 'group/exact&%!' });
  const config = path.join(repo, '.agents', 'dispatch.local.json');
  put(config, { node: process.execPath, cli, provider: 'fixture', model: 'group/exact&%!', thinking: 'medium', worktree_parent: parent });
  const spec = path.join(root, 'spec 空格 & % !.md');
  fs.writeFileSync(spec, '# Goal\nfixture\n# Scope\nlocal\n# Acceptance\nverified\n# Relevant docs\ndocs/README.md\n');
  const opts = { repo, config, task: 'sample', branch: 'chore/sample', base, spec, delivery: 'local-only', 'first-release': 'true' };
  const children = [];
  t.after(async () => {
    // Only fixture children receive a cooperative exit file. Never kill by discovered PID.
    for (const child of children) if (child.sessionDir && fs.existsSync(child.sessionDir)) fs.writeFileSync(path.join(child.sessionDir, 'exit-gate'), 'exit');
    await Promise.all(children.map(c => c.done));
    assert.ok(path.basename(root).startsWith('githux-dispatch-test-'));
    assert.equal(path.dirname(root), os.tmpdir());
    fs.rmSync(root, { recursive: true, force: true });
  });
  function launch(payload) {
    const encoded = Buffer.from(JSON.stringify(payload)).toString('base64');
    const code = `import {supervise} from ${JSON.stringify(new URL('./dispatch.mjs', import.meta.url).href)}; const p=JSON.parse(Buffer.from(process.argv[1],'base64')); await supervise(p.worktree,p.token,{allowNonTTY:true});`;
    const proc = spawn(process.execPath, ['--input-type=module', '-e', code, encoded], { stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true });
    let output = '';
    proc.stdout.on('data', d => output += d); proc.stderr.on('data', d => output += d);
    const record = { sessionDir: path.join(payload.worktree, '.agents', 'task', 'sessions'), done: new Promise(resolve => proc.on('exit', code => resolve({ code, output }))) };
    children.push(record);
    return record;
  }
  return { root, repo, parent, settings, config, spec, opts, children, launch };
}

test('create uses explicit committed base, isolated branch/context, local-only handoff and no private copies', t => {
  const f = fixture(t), s = createTask(f.opts);
  assert.equal(s.base_sha, f.opts.base);
  assert.equal(git(s.worktree, ['rev-parse', 'HEAD']), f.opts.base);
  assert.equal(git(s.worktree, ['branch', '--show-current']), 'chore/sample');
  assert.equal(fs.readFileSync(path.join(s.worktree, 'docs', 'README.md'), 'utf8'), 'committed shared facts');
  assert.equal(fs.existsSync(path.join(s.worktree, '.agents', 'secret.json')), false);
  assert.equal(fs.existsSync(path.join(s.worktree, 'untracked-private.txt')), false);
  assert.equal(fs.readFileSync(path.join(f.repo, 'docs', 'README.md'), 'utf8'), 'uncommitted facts must not copy');
  assert.equal(s.milestone, 1);
  assert.equal(s.delivery_scope, 'local-only');
  const handoff = fs.readFileSync(path.join(s.worktree, '.agents', 'task', 'TASK.md'), 'utf8');
  assert.match(handoff, /\.agents\/task\/implementation-result\.md/);
  assert.doesNotMatch(handoff, /(^|[^.\/])implementation-result\.md/m);
  assert.match(handoff, /delivery-state\.json/);
  const delivery = json(path.join(s.worktree, '.agents', 'task', 'delivery-state.json'));
  assert.equal(delivery.arbitration_ever_required, false);
  assert.equal(delivery.merge_owner, 'worker');
  assert.equal(s.arbitration_ever_required, undefined);
  assert.equal(taskStatus(s.worktree).process_state, 'created');
  // A missing worker file must fail loudly, never default arbitration to false.
  fs.renameSync(path.join(s.worktree, '.agents', 'task', 'delivery-state.json'), path.join(f.root, 'delivery.saved'));
  assert.throws(() => taskStatus(s.worktree), /ENOENT/);
  fs.renameSync(path.join(f.root, 'delivery.saved'), path.join(s.worktree, '.agents', 'task', 'delivery-state.json'));
  const malformed = json(path.join(s.worktree, '.agents', 'task', 'delivery-state.json'));
  put(path.join(s.worktree, '.agents', 'task', 'delivery-state.json'), { ...malformed, merge_owner: 'someone' });
  assert.throws(() => taskStatus(s.worktree), /Invalid worker-owned delivery state/);
  put(path.join(s.worktree, '.agents', 'task', 'delivery-state.json'), malformed);
  assert.throws(() => createTask(f.opts), /already exists/);
  assert.throws(() => createTask({ ...f.opts, task: 'another' }), /Branch already exists/);
  // A relative spec resolves from the explicitly validated repo root, never a shell cwd.
  const relative = createTask({ ...f.opts, task: 'relative-spec', branch: 'chore/relative-spec', spec: path.relative(f.repo, f.spec) });
  assert.equal(relative.base_sha, f.opts.base);
});

test('reject path escapes, unsupported config and malformed specs before mutations; unrelated tasks no milestone', t => {
  const f = fixture(t);
  for (const task of ['../escape', 'a/b', '..', 'NUL', 'a&evil']) assert.throws(() => createTask({ ...f.opts, task }), /task must/);
  assert.deepEqual(fs.readdirSync(f.parent), []);
  const cfg = json(f.config); put(f.config, { ...cfg, apiKey: 'fixture-secret-never-copy' });
  assert.throws(() => createTask(f.opts), /credentials/);
  put(f.config, cfg);
  assert.throws(() => createTask({ ...f.opts, base: '--evil' }), /Base cannot/);
  fs.writeFileSync(f.spec, '# Goal\nmissing fields');
  assert.throws(() => createTask(f.opts), /requires a Markdown heading/);
  fs.writeFileSync(f.spec, '# 目标\n任务\n# 范围\n本地\n# 验收\n检查\n# 相关文档\ndocs/README.md');
  const s = createTask({ ...f.opts, 'first-release': 'false', delivery: 'pr' });
  assert.equal(s.milestone, null);
  assert.equal(s.delivery_scope, 'pr');
});

test('inventory exact mismatch and failure refuse launch without lock or child', t => {
  const f = fixture(t), s = createTask(f.opts);
  let launched = false;
  for (const settings of [{ provider: 'fixture', model: 'group/exact&%!suffix' }, { provider: 'other', model: 'group/exact&%!' }, { inventoryFailure: true }]) {
    put(f.settings, settings);
    assert.throws(() => launchTask('start', { config: f.config, worktree: s.worktree }, () => { launched = true; }), /inventory/i);
    assert.equal(taskStatus(s.worktree).process_state, 'created');
    assert.equal(fs.existsSync(path.join(s.worktree, '.agents', 'task', 'launch.lock')), false);
  }
  assert.equal(launched, false);
});

test('real child lifecycle, repeat launch refused, exact resume preserves original context and PR/arbitration', async t => {
  const f = fixture(t), s = createTask(f.opts);
  const dir = path.join(s.worktree, '.agents', 'task');
  const stateFile = path.join(dir, 'dispatch-state.json');
  const deliveryFile = path.join(dir, 'delivery-state.json');
  const taskFile = path.join(dir, 'TASK.md');
  const original = fs.readFileSync(taskFile, 'utf8');
  launchTask('start', { config: f.config, worktree: s.worktree }, f.launch);
  const invocation = path.join(s.session_dir, 'invocations.jsonl');
  await until(() => fs.existsSync(invocation));
  const active = await until(() => { const v = taskStatus(s.worktree); return v.process_state === 'active' && v; });
  assert.ok(active.runtime.attempts[0].child.birth);
  // Deterministic overlap: atomically publish arbitration true/coordinator while runtime changes around child exit.
  const workerWrite = patch => {
    const current = json(deliveryFile);
    const tmp = `${deliveryFile}.${process.pid}.${Math.random()}.tmp`;
    put(tmp, { ...current, ...patch, updated_at: new Date().toISOString() });
    fs.renameSync(tmp, deliveryFile);
  };
  workerWrite({ arbitration_ever_required: true, merge_owner: 'coordinator', pr_url: 'https://example.invalid/pr/concurrent', progress: { task: 'awaiting-decision' } });
  const overlap = taskStatus(s.worktree);
  assert.equal(overlap.arbitration_ever_required, true);
  assert.equal(overlap.merge_owner, 'coordinator');
  assert.equal(overlap.pr_url, 'https://example.invalid/pr/concurrent');
  assert.throws(() => launchTask('start', { config: f.config, worktree: s.worktree }, f.launch), /lock/);
  assert.throws(() => launchTask('resume', { config: f.config, worktree: s.worktree, followup: f.spec }, f.launch), /lock/);
  const call = JSON.parse(fs.readFileSync(invocation, 'utf8').trim());
  assert.equal(call.cwd, s.worktree);
  assert.ok(call.args.includes('--approve'));
  assert.ok(!call.args.includes('--print'));
  assert.equal(call.args.at(-1), `@${taskFile}`);
  assert.equal(call.args[call.args.indexOf('--model') + 1], 'group/exact&%!');
  fs.writeFileSync(path.join(s.session_dir, 'exit-gate'), 'exit');
  assert.equal((await f.children[0].done).code, 0);
  let exited = taskStatus(s.worktree);
  assert.equal(exited.process_state, 'exited');
  // Exit writes runtime only; the worker's concurrent true/coordinator survives unchanged.
  assert.equal(exited.progress.task, 'awaiting-decision');
  assert.equal(exited.arbitration_ever_required, true);
  assert.equal(exited.merge_owner, 'coordinator');
  assert.equal(exited.pr_url, 'https://example.invalid/pr/concurrent');
  assert.equal(json(stateFile).arbitration_ever_required, undefined);
  assert.equal(json(deliveryFile).arbitration_ever_required, true);
  assert.ok(exited.session_path);
  assert.match(exited.diagnostic, /completion/);
  fs.unlinkSync(path.join(s.session_dir, 'exit-gate'));
  workerWrite({ pr_url: 'https://example.invalid/pr/7' });
  put(f.settings, { provider: 'fixture', model: 'new/model' });
  const followup = path.join(f.root, '补充 & % !.md');
  fs.writeFileSync(followup, 'Continue existing task, keep arbitration history.');
  const configBeforeResume = json(f.config);
  put(f.config, { ...configBeforeResume, provider: 'alternate' });
  put(f.settings, { provider: 'alternate', model: 'new/model' });
  let unexpectedLaunch = false;
  assert.throws(
    () => launchTask('resume', { config: f.config, worktree: s.worktree, followup, model: 'new/model' }, () => { unexpectedLaunch = true; }),
    /Provider changed.*--provider/
  );
  assert.equal(unexpectedLaunch, false);
  assert.equal(fs.existsSync(path.join(dir, 'launch.lock')), false);
  assert.equal(json(stateFile).runtime.attempts.length, 1);
  launchTask('resume', { config: f.config, worktree: s.worktree, followup, model: 'new/model', provider: 'alternate' }, f.launch);
  await until(() => fs.readFileSync(invocation, 'utf8').trim().split('\n').length === 2);
  const calls = fs.readFileSync(invocation, 'utf8').trim().split('\n').map(JSON.parse);
  assert.equal(calls[1].session, calls[0].session);
  assert.ok(calls[1].args.includes('--session'));
  assert.ok(!calls[1].args.includes('--session-id'));
  assert.equal(fs.readFileSync(taskFile, 'utf8'), original);
  assert.equal(fs.readFileSync(calls[1].args.at(-1).slice(1), 'utf8'), fs.readFileSync(followup, 'utf8'));
  fs.writeFileSync(path.join(s.session_dir, 'exit-gate'), 'exit');
  assert.equal((await f.children[1].done).code, 0);
  exited = taskStatus(s.worktree);
  assert.equal(exited.arbitration_ever_required, true);
  assert.equal(exited.merge_owner, 'coordinator');
  assert.equal(exited.pr_url, 'https://example.invalid/pr/7');
  assert.equal(exited.progress.task, 'awaiting-decision');
  assert.equal(exited.session_id, s.session_id);
  assert.deepEqual(exited.model_history.map(h => h.configured_model), ['group/exact&%!', 'new/model']);
  assert.deepEqual(exited.model_history.map(h => h.provider), ['fixture', 'alternate']);
  const cfg = json(f.config); put(f.config, { ...cfg, model: 'new/model' });
  const savedSession = fs.readFileSync(exited.session_path);
  fs.writeFileSync(exited.session_path, `${JSON.stringify({ type: 'session', id: 'wrong-id', cwd: s.worktree })}\n`);
  assert.throws(() => launchTask('resume', { config: f.config, worktree: s.worktree, followup }, f.launch), /session.*missing/i);
  fs.writeFileSync(exited.session_path, savedSession);
  fs.unlinkSync(exited.session_path);
  assert.throws(() => launchTask('resume', { config: f.config, worktree: s.worktree, followup }, f.launch), /session.*missing/i);
  assert.equal(fs.existsSync(path.join(dir, 'launch.lock')), false);
});

test('launcher failure leaves ambiguous lock; missing metadata and PID reuse never imply active/safe recovery', t => {
  const f = fixture(t), s = createTask(f.opts);
  assert.throws(() => launchTask('start', { config: f.config, worktree: s.worktree }, () => { throw new Error('fixture launcher crash'); }), /preserved/);
  assert.equal(taskStatus(s.worktree).process_state, 'unknown');
  assert.throws(() => launchTask('start', { config: f.config, worktree: s.worktree }, f.launch), /lock/);
  const dir = path.join(s.worktree, '.agents', 'task');
  fs.unlinkSync(path.join(dir, 'launch.lock', 'launch.json'));
  assert.equal(taskStatus(s.worktree).process_state, 'unknown');
  const p = path.join(dir, 'dispatch-state.json');
  const state = json(p);
  state.runtime.phase = 'active';
  state.runtime.attempts[0].supervisor = { pid: process.pid, birth: 'wrong-creation-time' };
  put(p, state);
  const status = taskStatus(s.worktree);
  assert.equal(status.observed.supervisor, false);
  assert.equal(status.process_state, 'unknown');
});

test('partial Git creation failure preserves reserved target and existing dirty coordinator', t => {
  const f = fixture(t);
  git(f.repo, ['update-ref', 'refs/heads/chore', f.opts.base]);
  assert.throws(() => createTask(f.opts), /Partial create preserved/);
  assert.equal(fs.existsSync(path.join(f.parent, 'githux-sample')), true);
  assert.equal(fs.readFileSync(path.join(f.repo, 'docs', 'README.md'), 'utf8'), 'uncommitted facts must not copy');
  assert.throws(() => createTask(f.opts), /already exists/);
});

test('nonzero child exit reports failed, offline CLI status JSON and missing session resume refuse', async t => {
  const f = fixture(t), s = createTask(f.opts);
  put(f.settings, { provider: 'fixture', model: 'group/exact&%!', exitCode: 5 });
  launchTask('start', { config: f.config, worktree: s.worktree }, f.launch);
  await until(() => fs.existsSync(path.join(s.session_dir, 'invocations.jsonl')));
  fs.writeFileSync(path.join(s.session_dir, 'exit-gate'), 'exit');
  assert.equal((await f.children[0].done).code, 0);
  const status = taskStatus(s.worktree);
  assert.equal(status.process_state, 'failed');
  assert.equal(status.runtime.attempts[0].code, 5);
  assert.equal(status.progress.task, 'pending');
  fs.unlinkSync(f.config); // status has no config or network dependency.
  const result = spawnSync(process.execPath, [script, 'status', '--worktree', s.worktree], { encoding: 'utf8', windowsHide: true });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(JSON.parse(result.stdout).process_state, 'failed');
});

test('Windows process identity and encoded bridge preserve shell characters without visible windows', { skip: process.platform !== 'win32' }, t => {
  const f = fixture(t);
  assert.equal(loadConfig(f.config).cli.includes('空格 & % !'), true);
  const row = processTable().find(r => r.pid === process.pid);
  assert.ok(row.birth && row.command);
  const bridge = path.join(f.root, 'bridge 空格 & % !.mjs');
  const bridgeOut = path.join(f.parent, 'bridge-result.json');
  fs.writeFileSync(bridge, `import fs from 'node:fs'; fs.writeFileSync('bridge-result.json',JSON.stringify({cwd:process.cwd(),args:process.argv.slice(2)}));`);
  const bridgeArgs = { node: process.execPath, script: bridge, worktree: f.parent, token: 'fixture-token', task: 'fixture' };
  const ps = path.join(process.env.SystemRoot, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  // Execute the exact encoded inner bridge hidden, with a harmless recorder in place of the supervisor.
  const roundtrip = spawnSync(ps, ['-NoProfile', '-NonInteractive', '-EncodedCommand', terminalCommand(bridgeArgs)], { encoding: 'utf8', windowsHide: true });
  assert.equal(roundtrip.status, 0, roundtrip.stderr);
  const recorded = json(bridgeOut);
  // Runner TEMP may use an 8.3 alias; cwd can resolve to the same directory's long name.
  assert.equal(fs.realpathSync.native(recorded.cwd), fs.realpathSync.native(f.parent));
  assert.deepEqual(recorded.args, ['__supervise', f.parent, 'fixture-token']);
  assert.throws(() => terminalCommand({ ...bridgeArgs, script: 'x'.repeat(10000) }), /CMD length/);
  const input = "路径 with spaces & %TEMP% ! bang $(evil) '";
  const encoded = encodePS(`$s=[System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('${Buffer.from(input).toString('base64')}')); [Console]::OutputEncoding=[System.Text.Encoding]::UTF8; [Console]::Write($s)`);
  const exe = path.join(process.env.SystemRoot, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  const result = spawnSync(exe, ['-NoProfile', '-NonInteractive', '-EncodedCommand', encoded], { encoding: 'utf8', windowsHide: true });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout.replace(/^\uFEFF/, ''), input);
});
