import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { classifyChanges, changedFiles, verifyJobs } from './ci-scope.mjs';

const docs = ['README.md', 'AGENTS.md', 'LESSONS.md', 'docs/roadmap.md', 'docs/images/ui.png',
  '.agents/skills/githux-pr-review/SKILL.md'];
test('documentation-only changes skip Rust and agent script jobs', () => {
  assert.deepEqual(classifyChanges(docs), { app: false, scripts: false });
  assert.deepEqual(classifyChanges([]), { app: false, scripts: false });
});
test('application, bundled resources and unknown paths require Rust, even after many docs', () => {
  for (const path of ['src/main.rs', 'src/README.md', 'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml',
    'build.rs', '.cargo/config.toml', 'assets/fonts/README.md', 'assets/icon.png', 'tests/test.rs',
    'future-crate/lib.rs', '.gitignore']) {
    assert.deepEqual(classifyChanges([...Array(400).fill('docs/file.md'), path]), { app: true, scripts: false });
  }
});
test('tooling-only changes run script checks; workflow/policy changes exercise all jobs', () => {
  assert.deepEqual(classifyChanges([...docs, '.agents/skills/example/scripts/check.mjs']), { app: false, scripts: true });
  assert.deepEqual(classifyChanges(['src/main.rs', '.agents/skills/example/fixtures/data.json']), { app: true, scripts: true });
  for (const path of ['.github/workflows/ci.yml', '.github/scripts/ci-scope.mjs'])
    assert.deepEqual(classifyChanges([path]), { app: true, scripts: true });
});

function results(app = false, scripts = false) {
  return { changes: { result: 'success', outputs: { app: String(app), scripts: String(scripts) } },
    'code-check': { result: app ? 'success' : 'skipped' }, build: { result: app ? 'success' : 'skipped' },
    'script-check': { result: scripts ? 'success' : 'skipped' } };
}
test('aggregate passes only intentional skips and successful required jobs', () => {
  for (const app of [false, true]) for (const scripts of [false, true]) verifyJobs(results(app, scripts));
  for (const key of ['code-check', 'build', 'script-check']) {
    for (const result of ['failure', 'cancelled', 'skipped', '', undefined]) {
      const needs = results(true, true); needs[key].result = result;
      assert.throws(() => verifyJobs(needs), /expected success/);
    }
    const needs = results(); needs[key].result = 'failure';
    assert.throws(() => verifyJobs(needs), /expected skipped/);
  }
});
test('aggregate rejects failed detector, missing/invalid outputs or omitted/unknown dependencies', () => {
  for (const result of ['failure', 'cancelled', 'skipped', undefined]) {
    const needs = results(); needs.changes.result = result;
    assert.throws(() => verifyJobs(needs), /detection did not succeed/);
  }
  for (const app of ['', undefined, true, 'FALSE']) {
    const needs = results(); needs.changes.outputs.app = app;
    assert.throws(() => verifyJobs(needs), /outputs/);
  }
  const absent = results(); delete absent.build;
  assert.throws(() => verifyJobs(absent), /missing/);
  assert.throws(() => verifyJobs({ ...results(), extra: { result: 'success' } }), /no gate policy/);
});

test('real Git detects full PR vs latest push, base advancement, renames/deletions and initial pushes', () => {
  const root = resolve(tmpdir());
  const dir = mkdtempSync(join(root, 'githux-ci-scope-'));
  const git = (...args) => execFileSync('git', args, { cwd: dir, encoding: 'utf8', windowsHide: true,
    stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  const write = (path, text) => { mkdirSync(dirname(join(dir, path)), { recursive: true }); writeFileSync(join(dir, path), text); };
  const commit = message => { git('add', '-A'); git('-c', 'commit.gpgsign=false', '-c', 'user.name=CI Test',
    '-c', 'user.email=ci-test@example.invalid', 'commit', '-qm', message); return git('rev-parse', 'HEAD'); };
  const pr = (base, head) => ({ pull_request: { base: { sha: base }, head: { sha: head } } });
  try {
    git('init', '-b', 'main');
    write('src/main.rs', 'fn main() {}\n'); write('docs/guide.md', 'initial\n');
    const base = commit('initial');
    git('checkout', '-b', 'feature'); write('src/main.rs', 'fn main() { println!("hi"); }\n');
    const code = commit('code'); write('docs/guide.md', 'changed\n'); const head = commit('docs');
    assert.deepEqual(classifyChanges(changedFiles('pull_request', pr(base, head), dir)), { app: true, scripts: false });
    assert.deepEqual(classifyChanges(changedFiles('push', { before: code, after: head }, dir)), { app: false, scripts: false });
    git('checkout', 'main'); write('assets/main-only.txt', 'base advanced'); const advanced = commit('base advance');
    git('checkout', '-b', 'docs-only', base); write('docs/guide.md', 'docs branch'); const docsHead = commit('docs-only');
    assert.deepEqual(changedFiles('pull_request', pr(advanced, docsHead), dir), ['docs/guide.md']);
    git('mv', 'src/main.rs', 'docs/moved.md'); const renamed = commit('rename source to docs');
    const names = changedFiles('push', { before: docsHead, after: renamed }, dir);
    assert.ok(names.includes('src/main.rs')); assert.ok(names.includes('docs/moved.md'));
    assert.equal(classifyChanges(names).app, true);
    assert.equal(classifyChanges(changedFiles('push', { before: '0'.repeat(40), after: head }, dir)).app, true);
    assert.throws(() => changedFiles('pull_request', pr('f'.repeat(40), head), dir));
    assert.throws(() => changedFiles('push', { before: '--evil', after: head }, dir), /invalid/);
    assert.throws(() => changedFiles('unknown', {}, dir), /Unsupported/);
  } finally {
    assert.equal(dirname(resolve(dir)), root); assert.ok(dir.startsWith(join(root, 'githux-ci-scope-')));
    rmSync(dir, { recursive: true, force: true });
  }
});
