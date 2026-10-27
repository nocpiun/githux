import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

// Exercise the exact metadata-only script shipped in the workflow, with no token/network.
const workflow = readFileSync(new URL('../workflows/pr.yml', import.meta.url), 'utf8').replace(/\r\n/g, '\n');
const marker = '          script: |\n';
assert.equal(workflow.split(marker).length, 2);
const script = workflow.split(marker)[1].trimEnd().split('\n').map(line => {
  assert.ok(line.startsWith('            '));
  return line.slice(12);
}).join('\n');
const run = new (Object.getPrototypeOf(async function () {}).constructor)('github', 'context', 'core', script);
const head = 'a'.repeat(40);
const pr = extra => ({ state: 'open', head: { sha: head }, base: { ref: 'main' }, labels: [], additions: 0, deletions: 0, ...extra });
async function check(data, event = pr(), error) {
  const failures = [], requests = [];
  await run({ rest: { pulls: { get: async request => {
    requests.push(request);
    if (error) throw error;
    return { data };
  } } } }, { repo: { owner: 'acme', repo: 'githux' }, issue: { number: 29 }, payload: { pull_request: event } },
  { info() {}, setFailed(message) { failures.push(message); } });
  assert.deepEqual(requests, [{ owner: 'acme', repo: 'githux', pull_number: 29 }]);
  return failures;
}

test('workflow refreshes the stable PR check on label changes and pushes, with read-only permissions', () => {
  assert.match(workflow, /^name: PR$/m);
  assert.match(workflow, /^    name: PR$/m);
  const events = workflow.match(/types: \[([^\]]+)\]/)[1].split(',').map(value => value.trim());
  for (const event of ['opened', 'reopened', 'synchronize', 'edited', 'labeled', 'unlabeled'])
    assert.ok(events.includes(event), `missing ${event} trigger`);
  assert.match(workflow, /pull-requests: read/);
  assert.match(workflow, /cancel-in-progress: true/);
  assert.doesNotMatch(workflow, /actions\/checkout|: write|\n\s+(paths|branches|if):/);
});

test('empty PR and inclusive independent 20000-line boundaries pass, including pending', async () => {
  for (const [additions, deletions] of [[0, 0], [20000, 0], [0, 20000], [20000, 20000]])
    assert.deepEqual(await check(pr({ additions, deletions, labels: [{ name: 'pending' }] })), []);
});

test('either total exceeding 20000 fails; additions and deletions never cancel each other', async () => {
  for (const [additions, deletions] of [[20001, 0], [0, 20001], [20001, 20001]]) {
    const result = await check(pr({ additions, deletions }));
    assert.equal(result.length, 1);
    if (additions > 20000) assert.match(result[0], /additions: 20001/);
    if (deletions > 20000) assert.match(result[0], /deletions: 20001/);
  }
});

test('blocked is an exact case-insensitive label; pending alone does not block', async () => {
  for (const name of ['blocked', 'Blocked', 'BLOCKED'])
    assert.match((await check(pr({ labels: [{ name: 'pending' }, { name }] })))[0], /blocked label/);
  assert.deepEqual(await check(pr({ labels: [{ name: 'unblocked' }, { name: 'blocked-by-ci' }] })), []);
});

test('re-run reads current labels and full PR totals rather than stale webhook metadata', async () => {
  assert.match((await check(pr({ labels: [{ name: 'blocked' }] }), pr()))[0], /blocked label/);
  assert.deepEqual(await check(pr(), pr({ labels: [{ name: 'blocked' }], additions: 50000 })), []);
  assert.match((await check(pr({ deletions: 20001 }), pr()))[0], /deletions: 20001/);
});

test('missing, malformed or failed metadata reads cannot pass', async () => {
  for (const labels of [undefined, null, {}, [null], [{ name: 3 }]])
    await assert.rejects(check(pr({ labels })), /labels/);
  for (const key of ['additions', 'deletions'])
    for (const value of [undefined, null, '0', -1, 0.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1])
      await assert.rejects(check(pr({ [key]: value })), new RegExp(key));
  await assert.rejects(check(pr(), pr(), new Error('API unavailable')), /API unavailable/);
});

test('old-head, retargeted or closed PR runs do not report success', async () => {
  for (const extra of [{ head: { sha: 'b'.repeat(40) } }, { base: { ref: 'release' } }, { state: 'closed' }])
    await assert.rejects(check(pr(extra)), /changed/);
});
