#!/usr/bin/env node
// Node 20+, no runtime dependencies. No GitHub mutations or credential handling.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';

const SELF = fileURLToPath(import.meta.url);
const fail = message => { throw new Error(message); };
const now = () => new Date().toISOString();
const exists = p => fs.existsSync(p);
const readJSON = p => JSON.parse(fs.readFileSync(p, 'utf8'));
function atomicJSON(p, value) {
  const tmp = `${p}.${randomUUID()}.tmp`;
  fs.writeFileSync(tmp, `${JSON.stringify(value, null, 2)}\n`, { flag: 'wx' });
  fs.renameSync(tmp, p);
}
function text(value, label) {
  if (typeof value !== 'string' || !value || /[\x00-\x1f\x7f"<>|]/u.test(value)) fail(`Invalid ${label}: empty, control characters, quotes, < > | are unsupported`);
  return value;
}
function optionPath(value, label) {
  text(value, label);
  if (!path.isAbsolute(value)) fail(`${label} must be absolute so the visible CMD does not depend on its cwd`);
  return value;
}
function utf8(p) {
  return new TextDecoder('utf-8', { fatal: true }).decode(fs.readFileSync(p));
}
function canonical(p) { return fs.realpathSync.native(path.resolve(text(p, 'path'))); }
function same(a, b) {
  const norm = p => process.platform === 'win32' ? path.resolve(p).toLowerCase() : path.resolve(p);
  return norm(a) === norm(b);
}
function inside(parent, target) {
  const rel = path.relative(parent, target);
  if (!rel || rel.startsWith('..') || path.isAbsolute(rel)) fail('Target must be strictly inside the intended worktree parent');
}
function plainDir(p) {
  if (!fs.lstatSync(p).isDirectory() || fs.lstatSync(p).isSymbolicLink()) fail(`Expected a real directory, not a link: ${p}`);
}
function run(exe, args, options = {}) {
  const result = spawnSync(exe, args, { encoding: 'utf8', windowsHide: true, timeout: 30000, maxBuffer: 8 * 1024 * 1024, ...options });
  if (result.error || result.status !== 0) fail(`Command failed: ${path.basename(exe)} (exit ${result.status ?? 'unknown'}). Inspect stderr; no state was deleted.`);
  return result.stdout;
}
function git(repo, args) { return run('git', ['-C', repo, ...args], { stdio: ['ignore', 'pipe', 'inherit'] }).trim(); }
function fields(obj, allowed, label) {
  if (!obj || typeof obj !== 'object' || Array.isArray(obj) || Object.keys(obj).some(k => !allowed.includes(k))) fail(`Unsupported ${label} fields; credentials, environment overrides and arbitrary CLI arguments are not accepted`);
}
function model(value, label) {
  text(value, label);
  if (!/^[a-zA-Z0-9][a-zA-Z0-9_./&%!\-]*$/u.test(value)) fail(`Unsupported ${label}; use an exact inventory ID without whitespace, colon suffixes or wildcards`);
  return value;
}
export function loadConfig(configFile, repo = process.cwd()) {
  const p = canonical(configFile ? optionPath(configFile, 'config path') : path.join(repo, '.agents', 'dispatch.local.json'));
  const c = readJSON(p);
  fields(c, ['node', 'cli', 'provider', 'model', 'thinking', 'worktree_parent'], 'configuration');
  if (c.node !== 'node' && !path.isAbsolute(text(c.node, 'node path'))) fail('node must be node or an absolute executable path');
  if (!path.isAbsolute(text(c.cli, 'CLI path'))) fail('cli must be an absolute JavaScript entry path');
  if (c.worktree_parent !== undefined && !path.isAbsolute(text(c.worktree_parent, 'worktree parent'))) fail('worktree_parent must be absolute');
  c.node = c.node === 'node' ? process.execPath : canonical(c.node);
  if (!path.isAbsolute(c.node) || (process.platform === 'win32' && path.extname(c.node).toLowerCase() !== '.exe')) fail('node must be node or an absolute Node executable');
  c.cli = canonical(c.cli);
  if (!/\.(?:m?js|cjs)$/i.test(c.cli) || !fs.statSync(c.cli).isFile()) fail('cli must be a JavaScript CLI entry file');
  c.provider = model(c.provider, 'provider');
  c.model = model(c.model, 'model');
  if (!['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'].includes(c.thinking)) fail('Invalid thinking level');
  c.worktree_parent = canonical(c.worktree_parent ?? 'D:/');
  plainDir(c.worktree_parent);
  return c;
}
export function checkInventory(c, cwd) {
  const result = spawnSync(c.node, [c.cli, '--offline', '--no-extensions', '--no-mcp', '--no-approve', '--list-models', c.model], {
    cwd, encoding: 'utf8', windowsHide: true, timeout: 30000, maxBuffer: 8 * 1024 * 1024, stdio: ['ignore', 'pipe', 'inherit'],
  });
  if (result.error || result.status !== 0) fail('Model inventory failed; launch refused (no fallback). Inspect executor stderr.');
  const rows = result.stdout.replace(/\x1b\[[0-9;]*m/g, '').split(/\r?\n/).map(line => line.trim().split(/\s+/));
  if (!rows.some(row => row.length >= 6 && row[0] === c.provider && row[1] === c.model)) fail('Exact provider/model absent from offline inventory; launch refused (no fallback).');
}
function locations(worktree) {
  const root = canonical(worktree);
  const dir = path.join(root, '.agents', 'task');
  plainDir(path.join(root, '.agents'));
  plainDir(dir);
  return { root, dir, state: path.join(dir, 'dispatch-state.json'), delivery: path.join(dir, 'delivery-state.json'), lock: path.join(dir, 'launch.lock') };
}
export function loadTask(worktree) {
  const loc = locations(worktree);
  const state = readJSON(loc.state);
  if (state.schema_version !== 1 || !same(state.worktree, loc.root) || !/^[a-z0-9][a-z0-9-]{0,63}$/.test(state.task_id) || !/^[a-f0-9-]{36}$/.test(state.session_id)) fail('Invalid task identity/state; inspect the preserved task directory');
  if (!same(state.session_dir, path.join(loc.dir, 'sessions'))) fail('Session directory is outside task context');
  plainDir(state.session_dir);
  if (state.session_path && !same(path.dirname(state.session_path), state.session_dir)) fail('Session path is outside the recorded session directory');
  // Delivery/PR/arbitration facts are worker-owned. Never merge them into, or rewrite them from, runtime updates.
  const delivery = readJSON(loc.delivery);
  fields(delivery, ['schema_version', 'task_id', 'arbitration_ever_required', 'merge_owner', 'pr_url', 'progress', 'updated_at'], 'delivery state');
  if (delivery.schema_version !== 1 || delivery.task_id !== state.task_id || typeof delivery.arbitration_ever_required !== 'boolean' || !['worker', 'coordinator'].includes(delivery.merge_owner) || !(delivery.pr_url === null || typeof delivery.pr_url === 'string') || !delivery.progress || typeof delivery.progress !== 'object' || Array.isArray(delivery.progress)) fail('Invalid worker-owned delivery state; inspect it instead of replacing arbitration/PR history');
  return { ...loc, stateData: state, deliveryData: delivery };
}
function update(worktree, mutate) {
  // Only supervisor-owned runtime/identity/model data lives here. This writer never touches delivery facts.
  const t = loadTask(worktree);
  const next = mutate(t.stateData) ?? t.stateData;
  atomicJSON(t.state, next);
  return next;
}
export function createTask(o) {
  const repo = canonical(o.repo ? optionPath(o.repo, 'repo') : process.cwd());
  if (!same(git(repo, ['rev-parse', '--show-toplevel']), repo)) fail('--repo must be the repository root');
  const c = loadConfig(o.config, repo);
  const id = o.task;
  if (typeof id !== 'string' || !/^[a-z0-9][a-z0-9-]{0,63}$/.test(id) || /^(con|prn|aux|nul|com[0-9]|lpt[0-9])$/i.test(id)) fail('task must be a portable lowercase task ID (1–64 letters/digits/hyphens)');
  if (typeof o.branch !== 'string' || !/^[a-z][a-z0-9-]*\/[a-z0-9][a-z0-9/-]*$/.test(o.branch)) fail('branch must be <type>/<branch-name> using lowercase letters, digits, slash and hyphen');
  git(repo, ['check-ref-format', '--branch', o.branch]);
  if (!['local-only', 'pr'].includes(o.delivery)) fail('--delivery must be local-only or pr');
  if (o['first-release'] !== undefined && !['true', 'false'].includes(o['first-release'])) fail('--first-release must be true or false');
  const baseInput = text(o.base, 'explicit base');
  if (baseInput.startsWith('-')) fail('Base cannot start with -');
  const sha = git(repo, ['rev-parse', '--verify', `${baseInput}^{commit}`]);
  if (!/^[a-f0-9]{40,64}$/.test(sha)) fail('Unable to resolve explicit base SHA');
  // Spec is read before any mutation; it may be relative only when it resolves from the validated repo root.
  const specPath = o.spec ? path.resolve(repo, o.spec) : '';
  const spec = utf8(canonical(optionPath(specPath, 'spec path')));
  for (const names of ['Goal|目标', 'Scope|范围', 'Acceptance|验收', 'Relevant docs|相关文档']) {
    if (!new RegExp(`^#{1,6}\\s+(?:${names})\\s*$`, 'mi').test(spec)) fail(`Task spec requires a Markdown heading: ${names}`);
  }
  const target = path.resolve(c.worktree_parent, `githux-${id}`);
  inside(c.worktree_parent, target);
  if (exists(target)) fail('Worktree target already exists; inspect partial creation, never overwrite or auto-delete');
  if (git(repo, ['branch', '--list', o.branch])) fail('Branch already exists; never reuse it implicitly');
  if (git(repo, ['ls-tree', '-r', '--name-only', sha, '--', '.agents/task'])) fail('Base tracks task-local files; refuse to overwrite them');
  // All caller input validated before the first mutation. mkdir reserves the target atomically.
  fs.mkdirSync(target);
  try {
    git(repo, ['worktree', 'add', '-b', o.branch, target, sha]);
    if (git(target, ['rev-parse', 'HEAD']) !== sha) fail('Worktree HEAD differs from resolved base');
    git(target, ['check-ignore', '.agents/task/dispatch-state.json']);
    git(target, ['check-ignore', '.agents/task/delivery-state.json']);
    const agents = path.join(target, '.agents');
    if (exists(agents)) plainDir(agents);
    else fs.mkdirSync(agents);
    const dir = path.join(agents, 'task');
    fs.mkdirSync(dir);
    plainDir(dir);
    const sessionDir = path.join(dir, 'sessions');
    fs.mkdirSync(sessionDir);
    const sessionId = randomUUID();
    const state = {
      schema_version: 1, task_id: id, repo, worktree: target, branch: o.branch, base_sha: sha,
      session_id: sessionId, session_dir: sessionDir, session_path: null,
      provider: c.provider, requested_model: c.model, configured_model: c.model, thinking: c.thinking,
      model_history: [], delivery_scope: o.delivery, milestone: o['first-release'] === 'true' ? 1 : null,
      runtime: { phase: 'created', attempts: [] }, created_at: now(),
    };
    const delivery = {
      schema_version: 1, task_id: id, arbitration_ever_required: false, merge_owner: 'worker', pr_url: null,
      progress: { task: 'pending', note: 'Process state does not establish task completion.' }, updated_at: now(),
    };
    const handoff = `# Dispatch handoff: ${id}\n\nWorktree: ${target}\nBranch: ${o.branch}\nCommitted base: ${sha}\nSession ID: ${sessionId}\nDelivery: ${o.delivery}\nFirst release milestone: ${state.milestone ?? 'not assigned'}\n\nRead AGENTS.md, docs/README.md, docs/code-standards.md and LESSONS.md from this committed worktree; then the relevant docs below.\nFollow [shared workflow](../../docs/agent-workflow.md), including latest-head CI/review, arbitration history, squash and default commit message rules. For PR review use [githux-pr-review](../skills/githux-pr-review/SKILL.md). Paths in this handoff resolve from .agents/task/TASK.md.\n${o.delivery === 'local-only' ? 'Implement and validate locally, write .agents/task/implementation-result.md, then stop generation while keeping the interactive terminal available. Do not commit, push or create a PR.' : 'Implement, validate, document and deliver through the explicitly scoped PR workflow. Preserve arbitration history in delivery-state.json and PR records; milestone #1 is required only when assigned above.'}\nDispatch performs no GitHub mutation. Task progress/PR/arbitration metadata belongs only in delivery-state.json (worker-owned); runtime belongs in dispatch-state.json (supervisor-owned). Never reset arbitration_ever_required or merge_owner on resume.\n\n---\n\n${spec}\n`;
    fs.writeFileSync(path.join(dir, 'TASK.md'), handoff, { flag: 'wx', encoding: 'utf8' });
    fs.writeFileSync(path.join(dir, 'dispatch-state.json'), `${JSON.stringify(state, null, 2)}\n`, { flag: 'wx' });
    fs.writeFileSync(path.join(dir, 'delivery-state.json'), `${JSON.stringify(delivery, null, 2)}\n`, { flag: 'wx' });
    return state;
  } catch (error) {
    fail(`Partial create preserved at ${target}: ${error.message}. Inspect git worktree list, branch and .agents/task before deliberate recovery.`);
  }
}

// Encoded PowerShell contains code plus base64 JSON, never interpolated shell values.
export const encodePS = source => Buffer.from(source, 'utf16le').toString('base64');
function powershell() {
  return path.join(process.env.SystemRoot ?? 'C:/Windows', 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
}
function psJSON(source) {
  return JSON.parse(run(powershell(), ['-NoProfile', '-NonInteractive', '-EncodedCommand', encodePS(`$ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue'; [Console]::OutputEncoding=[System.Text.Encoding]::UTF8; ${source}`)], { stdio: ['ignore', 'pipe', 'inherit'] }).replace(/^\uFEFF/, '') || '[]');
}
export function processTable() {
  if (process.platform === 'win32') {
    const rows = psJSON("@(Get-CimInstance Win32_Process | ForEach-Object { [PSCustomObject]@{pid=[int]$_.ProcessId; birth=$_.CreationDate.ToUniversalTime().ToString('o'); command=$_.CommandLine; executable=$_.ExecutablePath} }) | ConvertTo-Json -Compress");
    return Array.isArray(rows) ? rows : [rows];
  }
  if (process.platform !== 'linux') fail('Process identity inspection supports Windows and Linux only');
  const rows = [];
  for (const name of fs.readdirSync('/proc').filter(n => /^\d+$/.test(n))) {
    try {
      const stat = fs.readFileSync(`/proc/${name}/stat`, 'utf8');
      const parts = stat.slice(stat.lastIndexOf(')') + 2).split(' ');
      if (parts[0] === 'Z') continue;
      rows.push({ pid: Number(name), birth: parts[19], command: fs.readFileSync(`/proc/${name}/cmdline`, 'utf8').replaceAll('\0', ' ') });
    } catch (error) { if (!['ENOENT', 'ESRCH'].includes(error.code)) fail('Cannot inspect process identities; permissions may be insufficient'); }
  }
  return rows;
}
function identity(pid, table = processTable()) {
  const row = table.find(r => r.pid === pid);
  if (!row?.birth || !row.command) fail('Process identity/creation time unavailable; lock retained for inspection');
  return { pid: row.pid, birth: row.birth };
}
function alive(record, table) {
  if (!record?.pid || !record.birth) return 'unknown';
  const row = table.find(r => r.pid === record.pid);
  if (!row) return false;
  if (!row.birth) return 'unknown';
  return row.birth === record.birth;
}
function matchingSession(state, table) {
  // Conservative detection, including manual --session-id/--session/--resume-like attaches.
  return table.filter(r => r.pid !== process.pid && r.command && (r.command.includes(state.session_id) || (state.session_path && r.command.includes(state.session_path))));
}
export function taskStatus(worktree) {
  const t = loadTask(worktree), s = t.stateData, delivery = t.deliveryData;
  let table;
  try { table = processTable(); } catch { return { ...s, ...delivery, process_state: 'unknown', diagnostic: 'Process inventory unavailable; do not launch until identities can be checked.' }; }
  const last = s.runtime.attempts.at(-1);
  const supervisor = last?.supervisor ? alive(last.supervisor, table) : false;
  const child = last?.child ? alive(last.child, table) : false;
  const external = matchingSession(s, table).filter(r => r.pid !== last?.child?.pid);
  let phase = s.runtime.phase, diagnostic = 'Process state does not indicate reasoning activity or task completion.';
  const launchMeta = exists(path.join(t.lock, 'launch.json')) ? readJSON(path.join(t.lock, 'launch.json')) : null;
  const startingAge = Date.parse(s.runtime.starting_at ?? '');
  const recentStarting = phase === 'starting' && Number.isFinite(startingAge) && Date.now() - startingAge < 30000 && launchMeta?.token === last?.token;
  if (exists(t.lock)) {
    if (supervisor === true && child === true && phase === 'active') phase = 'active';
    else if (phase === 'starting' && (supervisor === true || recentStarting)) phase = 'starting';
    else {
      phase = 'unknown';
      diagnostic = 'Launch lock retained; missing metadata, stopped supervisor or orphan child is ambiguous. Inspect process identities and archive the lock deliberately only after every related process has stopped.';
    }
  } else if (supervisor === true || child === true || external.length || supervisor === 'unknown' || child === 'unknown') {
    phase = 'unknown'; diagnostic = 'Recorded or external session process exists without a valid active lock, or identity is inaccessible; stop/inspect before recovery.';
  } else if (!['created', 'exited', 'failed'].includes(phase)) {
    phase = 'unknown'; diagnostic = 'Runtime metadata is incomplete; inspect startup/exit records before recovery.';
  }
  return { ...s, ...delivery, process_state: phase, diagnostic, observed: { supervisor, child, possible_external_session_pids: external.map(r => r.pid), lock_present: exists(t.lock) } };
}
function stopped(s, table) {
  for (const a of s.runtime.attempts) {
    for (const key of ['launcher', 'supervisor', 'child']) {
      // A successful asynchronous launcher may still be returning in the coordinator process.
      if (key === 'launcher') continue;
      if (a[key] && alive(a[key], table) !== false) fail(`Previous ${key} is live or identity is unknown; quit the CLI and verify its process has stopped`);
    }
    if (a.child_spawn_pending) fail('A previous child spawn has incomplete identity metadata; inspect all session processes before deliberate state repair');
  }
  if (matchingSession(s, table).length) fail('Possible process attached to the same recorded session; quit it before launch');
}
export function findSession(s) {
  const files = fs.readdirSync(s.session_dir).filter(n => n.endsWith('.jsonl'));
  const matches = [];
  for (const file of files) {
    const p = path.join(s.session_dir, file);
    if (fs.lstatSync(p).isSymbolicLink() || !fs.statSync(p).isFile()) fail('Session file is not a regular file');
    const fd = fs.openSync(p, 'r');
    let head;
    try { const buf = Buffer.alloc(65536); const size = fs.readSync(fd, buf, 0, buf.length, 0); head = JSON.parse(buf.subarray(0, size).toString('utf8').split(/\r?\n/)[0]); }
    catch { fail('Unreadable session header; refuse to create or guess a session'); }
    finally { fs.closeSync(fd); }
    if (head.type === 'session' && head.id === s.session_id && typeof head.cwd === 'string' && same(head.cwd, s.worktree)) matches.push(p);
  }
  if (matches.length !== 1 || (s.session_path && !same(matches[0], s.session_path))) fail('Exact existing session is missing, duplicated or differs from recorded path; resume refused, never create a replacement');
  return matches[0];
}
export function terminalOuter(payload, inner) {
  return `$s=New-Object System.Diagnostics.ProcessStartInfo; $s.FileName=$env:ComSpec; $s.UseShellExecute=$true; $s.WorkingDirectory=[System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('${Buffer.from(payload.worktree).toString('base64')}')); $s.Arguments='/d /v:off /k powershell.exe -NoLogo -NoProfile -EncodedCommand ${inner}'; [System.Diagnostics.Process]::Start($s) | Out-Null`;
}
export function terminalCommand(payload) {
  const data = Buffer.from(JSON.stringify(payload), 'utf8').toString('base64');
  // Child inherits the visible terminal. Any final message stays readable; errors include the supervisor's exit.
  const inner = encodePS(`$ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue'; [Console]::OutputEncoding=[System.Text.Encoding]::UTF8; try { $p=[System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('${data}')) | ConvertFrom-Json; [Console]::Title=('Githux: '+$p.task); Set-Location -LiteralPath $p.worktree; & $p.node $p.script '__supervise' $p.worktree $p.token; if ($LASTEXITCODE -ne 0) { Write-Host ('Dispatch supervisor exited: '+$LASTEXITCODE) } } catch { Write-Host ('Dispatch failed: '+$_.Exception.Message); Write-Host ('State is preserved; run: node "'+$p.script+'" status --worktree "'+$p.worktree+'"') }`);
  // CMD command-line limit is smaller than CreateProcess. Reject before taking a lock.
  if (inner.length + 100 > 8000) fail('Encoded terminal command exceeds supported CMD length; shorten coordinator/script/worktree paths');
  return inner;
}
export function visibleLaunch(payload) {
  if (process.platform !== 'win32') fail('Visible interactive launch currently requires Windows CMD');
  const inner = terminalCommand(payload);
  const outer = terminalOuter(payload, inner);
  run(powershell(), ['-NoProfile', '-NonInteractive', '-EncodedCommand', encodePS(`$ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue'; ${outer}`)], { stdio: ['ignore', 'inherit', 'inherit'] });
}
export function launchTask(mode, o, launch = visibleLaunch) {
  if (!['start', 'resume'].includes(mode)) fail('Invalid launch command');
  const t = loadTask(optionPath(o.worktree, 'worktree')), s = t.stateData;
  const c = loadConfig(o.config, s.repo);
  // Explicit model override on resume records the caller's authorized choice.
  if (o.model !== undefined) {
    if (mode !== 'resume') fail('--model override is supported only for explicit resume');
    c.model = model(o.model, 'model');
  }
  if (o.provider !== undefined) {
    if (mode !== 'resume') fail('--provider override is supported only for explicit resume');
    c.provider = model(o.provider, 'provider');
  }
  if (mode === 'resume' && o.model === undefined && c.model !== s.configured_model) fail('Model changed; authorize explicitly with --model on resume');
  if (mode === 'resume' && o.provider === undefined && c.provider !== s.provider) fail('Provider changed; authorize explicitly with --provider on resume');
  if (mode === 'start' && (c.model !== s.configured_model || c.provider !== s.provider)) fail('Config model changed since create; restore it or use explicit resume once a session exists');
  if (launch === visibleLaunch && process.platform !== 'win32') fail('Visible launch requires Windows; no terminal or lock was created');
  if (exists(t.lock)) fail('Launch lock already exists; active or stale/ambiguous launches cannot be started again. Inspect status and recovery reference.');
  const phase = taskStatus(t.root).process_state;
  if (mode === 'start' && phase !== 'created') fail('start is only for an untouched created task; use resume with its exact existing session');
  if (mode === 'resume' && !['exited', 'failed'].includes(phase)) fail('resume requires a confirmed stopped task; inspect/reconcile unknown runtime state deliberately');
  stopped(s, processTable());
  const session = mode === 'resume' ? findSession(s) : null;
  if (mode === 'start' && fs.readdirSync(s.session_dir).length) fail('Session directory is not empty; refuse to overwrite/reuse a session');
  if (mode === 'start' && s.session_path) fail('Dispatch state already records a session; start would not create a genuinely new one');
  if (git(t.root, ['branch', '--show-current']) !== s.branch) fail('Worktree branch changed; restore the recorded branch before launch');
  let followup = null;
  if (mode === 'resume') {
    if (!o.followup) fail('resume requires --followup pointing to a UTF-8 instruction file');
    followup = utf8(canonical(optionPath(o.followup, 'followup path')));
    if (!followup.trim()) fail('Follow-up must not be empty');
    // Record the exact validated session before startup so live attach detection can see --session PATH.
    update(t.root, latest => { latest.session_path = session; return latest; });
  } else if (o.followup) fail('--followup applies to resume only');
  checkInventory(c, t.root);
  const token = randomUUID();
  const payload = { node: process.execPath, script: SELF, worktree: t.root, token, task: s.task_id };
  if (launch === visibleLaunch) terminalCommand(payload);
  // Exclusive lock spans terminal startup, supervisor and child lifetime. Never auto-reclaim.
  fs.mkdirSync(t.lock);
  try {
    const launcher = identity(process.pid);
    const followupPath = followup === null ? null : path.join(t.dir, `FOLLOWUP-${token}.md`);
    if (followupPath) fs.writeFileSync(followupPath, followup, { flag: 'wx', encoding: 'utf8' });
    atomicJSON(path.join(t.lock, 'launch.json'), { token, mode, config: c, requested_model: c.model, session, followup: followupPath, launcher, created_at: now() });
    update(t.root, latest => {
      latest.runtime.phase = 'starting';
      latest.runtime.starting_at = now();
      latest.runtime.attempts.push({ token, mode, launcher, requested_at: now(), supervisor: null, child: null, child_spawn_pending: false });
      return latest;
    });
    launch(payload);
    return taskStatus(t.root);
  } catch (error) {
    // Even a failed launcher could have created a terminal. Preserve lock, do not guess.
    try { update(t.root, latest => { latest.runtime.phase = 'unknown'; return latest; }); } catch { /* metadata itself may be missing; keep original evidence */ }
    fail(`Launch incomplete; lock and context preserved. Run status and inspect terminal/processes before recovery. ${error.message}`);
  }
}
export async function supervise(worktree, token, { allowNonTTY = false } = {}) {
  const t = loadTask(worktree);
  const metaPath = path.join(t.lock, 'launch.json');
  const meta = readJSON(metaPath);
  if (meta.token !== token || t.stateData.runtime.attempts.at(-1)?.token !== token) fail('Launch token/state mismatch; refuse to claim lock');
  // Atomic supervisor claim also protects against replay of the same encoded terminal command.
  fs.writeFileSync(path.join(t.lock, 'supervisor.claim'), `${process.pid}\n`, { flag: 'wx' });
  const supervisor = identity(process.pid);
  update(t.root, s => { s.runtime.attempts.at(-1).supervisor = supervisor; return s; });
  if (!allowNonTTY && (!process.stdin.isTTY || !process.stdout.isTTY)) fail('Interactive inherited TTY required; lock retained, no print/RPC fallback');
  stopped({ ...t.stateData, runtime: { attempts: t.stateData.runtime.attempts.slice(0, -1) } }, processTable());
  // Revalidate in the terminal: there may have been a delay after the initial inventory.
  checkInventory(meta.config, t.root);
  const current = loadTask(t.root).stateData;
  if (meta.mode === 'resume' && !same(findSession(current), meta.session)) fail('Session changed before child startup');
  if (meta.mode === 'start' && fs.readdirSync(current.session_dir).length) fail('Unexpected session exists before first startup');
  const c = meta.config;
  const args = [c.cli, '--offline', '--approve', '--no-extensions', '--no-mcp', '--provider', c.provider, '--model', c.model, '--thinking', c.thinking, '--session-dir', current.session_dir];
  args.push(...(meta.mode === 'resume' ? ['--session', meta.session] : ['--session-id', current.session_id]));
  args.push(`@${meta.followup ?? path.join(t.dir, 'TASK.md')}`);
  update(t.root, s => {
    s.provider = c.provider; s.requested_model = meta.requested_model; s.configured_model = c.model; s.thinking = c.thinking;
    s.model_history.push({ at: now(), mode: meta.mode, provider: c.provider, requested_model: meta.requested_model, configured_model: c.model, backend_model: 'unverified' });
    s.runtime.attempts.at(-1).child_spawn_pending = true;
    return s;
  });
  const child = spawn(c.node, args, { cwd: t.root, stdio: 'inherit', windowsHide: false });
  // Install lifecycle listeners before identity inspection. Never kill a child on metadata failure.
  const ended = new Promise(resolve => {
    child.once('error', () => resolve({ code: null, signal: null, spawn_error: true }));
    child.once('exit', (code, signal) => resolve({ code, signal, spawn_error: false }));
  });
  // Ctrl-C belongs to the interactive child too; supervisor must wait for actual child exit.
  const ignoreSignal = () => {};
  process.on('SIGINT', ignoreSignal); process.on('SIGTERM', ignoreSignal);
  let childIdentity = null;
  try { if (child.pid) childIdentity = identity(child.pid); } catch { /* exit or inspection failure; exit handler below can settle it safely */ }
  update(t.root, s => {
    const a = s.runtime.attempts.at(-1); a.child = childIdentity;
    if (childIdentity) { a.child_spawn_pending = false; s.runtime.phase = 'active'; }
    return s;
  });
  const result = await ended;
  process.removeListener('SIGINT', ignoreSignal); process.removeListener('SIGTERM', ignoreSignal);
  let sessionPath = null;
  try { sessionPath = findSession(loadTask(t.root).stateData); } catch { /* no persisted session is explicit on resume */ }
  update(t.root, s => {
    const a = s.runtime.attempts.at(-1);
    Object.assign(a, result, { exited_at: now(), child_spawn_pending: false });
    s.runtime.phase = result.spawn_error || result.code !== 0 ? 'failed' : 'exited';
    if (sessionPath) s.session_path = sessionPath;
    return s;
  });
  if (readJSON(metaPath).token !== token) fail('Lock token changed; refuse to remove it');
  // Remove only our three known lock files, never recursively clean anything.
  fs.unlinkSync(path.join(t.lock, 'supervisor.claim'));
  fs.unlinkSync(metaPath);
  fs.rmdirSync(t.lock);
  console.log(`Githux CLI ${result.spawn_error ? 'failed to spawn' : `exited (${result.code ?? result.signal})`}; task completion remains unverified. Terminal stays available.`);
  return result;
}
function options(args, allowed) {
  const out = {};
  for (let i = 0; i < args.length; i += 2) {
    const key = args[i]?.slice(2);
    if (!args[i]?.startsWith('--') || !allowed.includes(key) || out[key] !== undefined || !args[i + 1] || args[i + 1].startsWith('--')) fail('Unknown, duplicate or missing command option; read references/operations.md');
    out[key] = args[i + 1];
  }
  return out;
}
async function main(args) {
  const [cmd, ...rest] = args;
  if (cmd === '__supervise') { if (rest.length !== 2) fail('Invalid internal supervisor invocation'); await supervise(...rest); return; }
  let result;
  if (cmd === 'create') result = createTask(options(rest, ['repo', 'config', 'task', 'branch', 'base', 'spec', 'delivery', 'first-release']));
  else if (cmd === 'status') result = taskStatus(optionPath(options(rest, ['worktree']).worktree, 'worktree'));
  else if (cmd === 'start' || cmd === 'resume') result = launchTask(cmd, options(rest, ['worktree', 'config', ...(cmd === 'resume' ? ['followup', 'model', 'provider'] : [])]));
  else fail('Usage: node dispatch.mjs create|start|status|resume [options]. See references/operations.md');
  console.log(JSON.stringify(result, null, 2));
}
if (process.argv[1] && same(process.argv[1], SELF)) {
  main(process.argv.slice(2)).catch(error => { console.error(`Dispatch error: ${error.message}`); process.exitCode = 1; });
}
