import test from 'node:test';
import { execFileSync } from 'node:child_process';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { parseArgs, inspectReview, waitForReview, saveResult, main, postReviewRetry, createGithubApi } from './wait-review.mjs';
import { inspectCI, checkoutRevision } from './ci-evidence.mjs';

const A = 'a'.repeat(40), B = 'b'.repeat(40), C = 'c'.repeat(40), M = 'd'.repeat(40);
const BOT = 'chatgpt-codex-connector[bot]';
const since = '2026-10-25T10:00:00Z', stamp = '2026-10-25T10:00:10Z';
const args = ['--repo', 'acme/githux', '--pr', '42', '--head', A];
const options = extra => ({ ...parseArgs(args), autoRetry: false, ...extra });
const bot = { login: BOT, type: 'Bot' };
const pr = extra => ({ state: 'open', head: { sha: A }, base: { ref: 'main', sha: B },
  created_at: since, mergeable: true, mergeable_state: 'clean', ...extra });
const review = extra => ({ id: 12, commit_id: A, submitted_at: stamp, user: bot, state: 'COMMENTED', body: '', ...extra });
const comment = extra => ({ id: 50, node_id: 'root50', user: bot, body: 'Finding', commit_id: A,
  pull_request_review_id: 12, created_at: stamp, ...extra });
function summary({ status = 'completed', head = A, findings = false, unresolved = findings,
  at = stamp, trigger = 'Manual request', extra = {} } = {}) {
  return { id: 20, user: bot, performed_via_github_app: { id: 1144995 }, created_at: since, updated_at: at,
    body: '<!-- codex-pull-request-review-summary -->\n<!-- codex-review-state:v1 ' +
      JSON.stringify({ code_review: { commit_sha: head, status, has_any_findings: findings, has_unresolved_findings: unresolved } }) +
      ' -->\n| 📝 **Code Review** | Completed <relative-time datetime="' + at + '">time</relative-time> | `' + head.slice(0, 7) + '` | ' + trigger + ' |', ...extra };
}
const receipt = extra => ({ id: 30, user: bot, performed_via_github_app: { id: 1144995 }, created_at: stamp,
  body: '<!-- codex-review-receipt:v1 -->\n\nCodex Review: Didn\'t find any major issues. Nice work!\n\n**Reviewed commit:** `' + A.slice(0, 10) + '`', ...extra });
const run = extra => ({ id: 70, workflow_id: 44, check_suite_id: 55, run_number: 7, run_attempt: 1,
  repository: { full_name: 'acme/githux' }, path: '.github/workflows/ci.yml', event: 'pull_request',
  head_sha: A, status: 'completed', conclusion: 'success', html_url: 'https://example.test/run/70',
  pull_requests: [{ number: 42, head: { sha: A }, base: { ref: 'main', sha: B } }], ...extra });
const job = extra => ({ id: 91, name: 'CI', head_sha: A, run_id: 70, run_attempt: 1,
  status: 'completed', conclusion: 'success', check_run_url: 'https://api.github.com/repos/acme/githux/check-runs/91', ...extra });
const checkoutLog = sha => '2026-10-25T10:00:00Z [command]/usr/bin/git log -1 --format=%H\n2026-10-25T10:00:01Z ' + sha + '\n';
function fixture(settings = {}) {
  const calls = [];
  let headReads = 0, runReads = 0, baseReads = 0;
  const api = async (path, config = {}) => {
    calls.push({ path, config });
    const s = typeof settings === 'function' ? settings() : settings;
    if (path.endsWith('/pulls/42')) return structuredClone((s.heads ?? [pr()])[Math.min(headReads++, (s.heads ?? [pr()]).length - 1)]);
    if (path.includes('/git/ref/heads/')) {
      const targets = s.targets ?? (s.heads ?? [pr()]).map(p => p.base.sha);
      return { ref: 'refs/heads/main', object: { type: 'commit', sha: targets[Math.min(baseReads++, targets.length - 1)] } };
    }
    if (path.includes('/issues/42/comments')) return structuredClone(s.comments ?? []);
    if (path.includes('/pulls/42/reviews?')) return structuredClone(s.reviews ?? []);
    if (/\/reviews\/\d+\/comments/.test(path)) return structuredClone(s.reviewInline ?? s.inline ?? []);
    if (path.includes('/pulls/42/comments?')) return structuredClone(s.inline ?? []);
    if (path === 'graphql') {
      const pages = s.threadPages ?? [{ nodes: (s.inline ?? []).filter(c => !c.in_reply_to_id).map(c => ({
        id: 'thread' + c.id, isResolved: c.resolved ?? false, isOutdated: c.outdated ?? false,
        comments: { nodes: [{ id: c.node_id }] },
      })), pageInfo: { hasNextPage: false } }];
      const page = /after:"page(\d+)"/.exec(config.query)?.[1] ?? '0';
      return { data: { repository: { pullRequest: { reviewThreads: structuredClone(pages[Number(page)]) } } } };
    }
    if (path.endsWith('/actions/workflows/ci.yml')) return s.workflow ?? { id: 44, path: '.github/workflows/ci.yml' };
    if (path.includes('/actions/workflows/44/runs?')) {
      const groups = s.runPages ?? [s.runs ?? [run()]];
      return structuredClone(groups[Math.min(runReads++, groups.length - 1)]);
    }
    if (/\/actions\/runs\/\d+\/attempts\/\d+\/jobs/.test(path)) return structuredClone(s.jobs ?? [job()]);
    if (path.includes('/check-runs/')) {
      const id = Number(path.split('/').at(-1));
      const j = (s.jobs ?? [job()]).find(j => j.check_run_url?.endsWith('/' + id));
      return { id, name: j?.name, head_sha: A, app: { id: 15368 }, check_suite: { id: 55 },
        status: j?.status, conclusion: j?.conclusion, ...s.check };
    }
    if (path.endsWith('/logs')) return s.log ?? checkoutLog(M);
    if (path.includes('/commits/')) return s.commit ?? { sha: M, parents: [{ sha: B }, { sha: A }] };
    throw new Error('Unexpected request: ' + path);
  };
  return { api, calls };
}
async function harness(t, extra = {}) {
  const dir = await mkdtemp(join(tmpdir(), 'githux-review-flow-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  let clock = Date.parse(since);
  const posts = [], sleeps = [], messages = [];
  const opts = options({ timeoutMs: 10000, intervalMs: 1000, evidenceGraceMs: 2000,
    retryState: join(dir, 'retry.json'), ...extra });
  const deps = { now: () => clock, sleep: async ms => { sleeps.push(ms); clock += ms; }, progress: s => messages.push(s),
    postRetry: async received => { posts.push(received); return { id: 100, created_at: new Date(clock).toISOString() }; } };
  return { dir, opts, deps, posts, sleeps, messages, now: () => clock, set: value => { clock = value; } };
}

// Identity and complete review batches.
test('arguments preserve required CI, validate boundaries and use one hour by default', () => {
  assert.equal(parseArgs(args).timeoutMs, 3600000);
  assert.equal(parseArgs(args).evidenceGraceMs, 120000);
  assert.equal(parseArgs(args).autoRetry, true);
  assert.deepEqual(parseArgs([...args, '--wait-ci', 'true', '--check', 'Other']).checks, ['CI', 'Other']);
  for (const tail of [['--head', B], ['--foo', 'bar'], ['--timeout-seconds', '-1'], ['--interval-seconds', '0'],
    ['--since', 'tomorrow'], ['--reviewer', 'bot;evil'], ['--auto-retry', 'yes'], ['--check', 'Other'],
    ['--evidence-grace-seconds', '301'], ['--after-comment-id', '30']]) assert.throws(() => parseArgs([...args, ...tail]));
  assert.equal(parseArgs([...args, '--timeout-seconds', '0']).timeoutMs, 0);
});

test('clean summary or recognized trusted receipt can complete this head without inline comments', async () => {
  for (const comments of [[summary()], [receipt()], [summary({ findings: true, unresolved: false })]]) {
    const r = await inspectReview(options(), fixture({ comments }).api);
    assert.equal(r.status, 'review_completed');
    assert.equal(r.review.state, 'clean');
    assert.equal(r.review_passed, null);
  }
});

test('PR14 regression: clean current receipt plus unresolved old threads returns history work immediately', async () => {
  const old = comment({ commit_id: C, pull_request_review_id: 8, created_at: since, outdated: true });
  const reply = comment({ id: 51, node_id: 'reply51', in_reply_to_id: 50, user: { login: 'maintainer' }, body: 'Discuss this tradeoff' });
  const f = fixture({ comments: [summary({ findings: true }), receipt()], inline: [old, reply] });
  const r = await inspectReview(options({ waitCi: true }), f.api);
  assert.equal(r.status, 'history_pending');
  assert.equal(r.review.state, 'clean');
  assert.equal(r.ci.state, 'success');
  assert.equal(r.threads.current.length, 0);
  assert.equal(r.threads.historical[0].outdated, true);
  assert.equal(r.threads.historical[0].comments[1].user.login, 'maintainer');
  assert.ok(r.next_actions.includes('verify_historical_threads'));
  assert.ok(f.calls.every(c => !c.config.query || c.config.query.startsWith('query ')));
});

test('body-only feedback is delivered with concurrent CI failure', async () => {
  const r = await inspectReview(options({ waitCi: true }), fixture({ comments: [summary({ findings: true })],
    reviews: [review({ body: 'The error path drops data' })], jobs: [job({ conclusion: 'failure' })],
    runs: [run({ conclusion: 'failure' })] }).api);
  assert.equal(r.status, 'review_arrived');
  assert.equal(r.review.reviews[0].comments.length, 0);
  assert.equal(r.ci.state, 'failed');
  assert.deepEqual(r.next_actions, ['assess_review', 'fix_ci']);
});

test('formal COMMENTED/APPROVED alone delivers evidence and never claims clean or merge permission', async () => {
  for (const state of ['COMMENTED', 'APPROVED', 'CHANGES_REQUESTED']) {
    const r = await inspectReview(options(), fixture({ reviews: [review({ state })] }).api);
    assert.equal(r.status, 'review_arrived');
    assert.equal(r.review_passed, null);
    assert.equal(r.verdict, 'not_evaluated');
  }
});

test('old, pending, dismissed, consumed, foreign-author and wrong-head reviews are ignored', async () => {
  const reviews = [review({ state: 'PENDING' }), review({ state: 'DISMISSED' }), review({ commit_id: B }),
    review({ id: 2 }), review({ submitted_at: since }), review({ user: { login: 'human' } })];
  assert.equal(await inspectReview(options({ since: stamp, afterReviewId: 10 }), fixture({ reviews }).api), null);
});

test('edited summary uses code-review time and SHA, not its original ID or security update', async () => {
  const opts = options({ since: stamp, afterCommentId: 25 });
  assert.equal((await inspectReview(opts, fixture({ comments: [summary()] }).api)).status, 'review_completed');
  for (const c of [summary({ head: B }), summary({ at: since, extra: { updated_at: stamp } })])
    assert.equal(await inspectReview(opts, fixture({ comments: [c] }).api), null);
});

test('unknown schema, identity, trigger or ordinary bot errors surface and do not pass', async () => {
  const invalid = [summary({ extra: { performed_via_github_app: { id: 999 } } }), summary({ status: 'other' }),
    summary({ unresolved: null }), summary({ findings: false, unresolved: true }), summary({ trigger: 'PR opened' }),
    summary({ extra: { body: '<!-- codex-pull-request-review-summary -->unknown' } }),
    receipt({ body: 'To use Codex here, create an environment' }), receipt({ body: '<!-- codex-review-receipt:v1 --> Unknown result' })];
  for (const c of invalid) assert.equal((await inspectReview(options({ since, afterCommentId: 10 }), fixture({ comments: [c] }).api)).status, 'unverified_bot_message');
  assert.equal(await inspectReview(options(), fixture({ comments: [summary({ extra: { user: { login: 'human' } } })] }).api), null);
});

test('prior round error does not poison explicitly retriggered review', async () => {
  const error = receipt({ id: 22, created_at: since, body: 'Environment unavailable' });
  const r = await inspectReview(options({ since: stamp, afterCommentId: 25 }), fixture({ comments: [error, summary()] }).api);
  assert.equal(r.status, 'review_completed');
});

test('clean summary conflicting with current changes requested is surfaced after bounded grace', async t => {
  const h = await harness(t);
  const api = fixture({ comments: [summary()], reviews: [review({ state: 'CHANGES_REQUESTED' })] }).api;
  const r = await waitForReview(h.opts, { ...h.deps, api });
  assert.equal(r.status, 'evidence_conflict');
  assert.equal(r.propagation_grace_expired, true);
  assert.equal(h.now() - Date.parse(since), 2000);
});

test('completed summary without its feedback returns evidence_incomplete, never an hour-long wait', async t => {
  const h = await harness(t);
  const r = await waitForReview(h.opts, { ...h.deps, api: fixture({ comments: [summary({ findings: true })] }).api });
  assert.equal(r.status, 'evidence_incomplete');
  assert.equal(h.now() - Date.parse(since), 2000);
  assert.equal(h.posts.length, 0);
});

test('propagated body-only feedback exits during grace; running batches wait for completion', async t => {
  for (const initial of ['running', 'completed']) {
    const h = await harness(t);
    const api = fixture(() => ({ comments: [summary({ findings: true, status: h.now() > Date.parse(since) ? 'completed' : initial })],
      reviews: h.now() > Date.parse(since) || initial === 'running' ? [review({ body: 'Fix this path' })] : [] })).api;
    const r = await waitForReview(h.opts, { ...h.deps, api });
    assert.equal(r.status, 'review_arrived');
    assert.equal(h.now() - Date.parse(since), 1000);
  }
});

test('receipt and still-running summary are a bounded propagation conflict', async t => {
  const h = await harness(t);
  const r = await waitForReview(h.opts, { ...h.deps, api: fixture({ comments: [summary({ status: 'running' }), receipt()] }).api });
  assert.equal(r.status, 'evidence_conflict');
  assert.equal(h.now() - Date.parse(since), 2000);
});

test('thread pagination keeps unresolved history and human replies, excluding resolved threads', async () => {
  const inline = [comment({ commit_id: B, pull_request_review_id: 8 }), comment({ id: 51, node_id: 'root51', commit_id: B }),
    comment({ id: 52, node_id: 'reply52', in_reply_to_id: 50, user: { login: 'owner' } })];
  const pages = [{ nodes: [{ id: 't51', isResolved: true, comments: { nodes: [{ id: 'root51' }] } }],
    pageInfo: { hasNextPage: true, endCursor: 'page1' } },
  { nodes: [{ id: 't50', isResolved: false, comments: { nodes: [{ id: 'root50' }] } }], pageInfo: { hasNextPage: false } }];
  const r = await inspectReview(options(), fixture({ comments: [receipt()], inline, threadPages: pages }).api);
  assert.equal(r.threads.historical.length, 1);
  assert.deepEqual(r.threads.historical[0].comments.map(c => c.id), [50, 52]);
});

test('missing thread roots are evidence gaps; malformed pagination is an error', async () => {
  const page = { nodes: [{ isResolved: false, comments: { nodes: [{ id: 'missing' }] } }], pageInfo: { hasNextPage: false } };
  assert.equal((await inspectReview(options(), fixture({ comments: [receipt()], threadPages: [page] }).api)).status, 'evidence_incomplete');
  await assert.rejects(inspectReview(options(), fixture({ threadPages: [{ nodes: [], pageInfo: { hasNextPage: true, endCursor: null } }] }).api), /cursor/);
});

test('CI-only wait stays in script; historical work is handed back before CI finishes', async t => {
  const h = await harness(t, { waitCi: true });
  const api = fixture(() => ({ comments: [summary()],
    runs: [run({ status: h.now() > Date.parse(since) ? 'completed' : 'in_progress' })] })).api;
  assert.equal((await waitForReview(h.opts, { ...h.deps, api })).status, 'checks_ready');
  const r = await inspectReview(options({ waitCi: true }), fixture({ comments: [receipt()],
    runs: [run({ status: 'in_progress' })], inline: [comment({ commit_id: B, pull_request_review_id: 1 })] }).api);
  assert.equal(r.status, 'history_pending');
  assert.equal(r.ci.state, 'pending');
});

// CI provenance, latest attempt, actual tested baseline and integration guards.
test('CI succeeds only with trusted workflow, attempt jobs, aggregate check and actual merge parents', async () => {
  const r = await inspectCI(options(), pr(), fixture().api);
  assert.equal(r.state, 'success');
  assert.equal(r.run.attempt, 1);
  assert.equal(r.merge_sha, M);
  assert.equal(r.tested_base, B);
});

test('CI aggregate cannot be removed by additional requested check names', async () => {
  const r = await inspectCI(options({ checks: ['Other'] }), pr(), fixture({ jobs: [job({ name: 'Other' })] }).api);
  assert.equal(r.state, 'unverified');
  assert.ok(r.checks.some(c => c.name === 'CI' && c.reason === 'missing'));
});

test('latest run wins, never older success or wrong event/workflow/PR', async () => {
  const other = [run({ id: 90, run_number: 9, event: 'push' }), run({ id: 91, run_number: 10, path: '.github/workflows/other.yml' }),
    run({ id: 92, run_number: 11, pull_requests: [{ number: 99, head: { sha: A }, base: { ref: 'main' } }] })];
  const r = await inspectCI(options(), pr(), fixture({ runs: [run(), ...other, run({ id: 71, run_number: 8, status: 'in_progress' })],
    jobs: [job({ run_id: 71, status: 'in_progress', conclusion: null })] }).api);
  assert.equal(r.state, 'pending');
  assert.equal(r.run.id, 71);
});

test('new attempt cannot reuse old attempt jobs, checks, or duplicate aggregate names', async () => {
  for (const settings of [{ runs: [run({ run_attempt: 2 })] }, { check: { app: { id: 999 } } },
    { check: { check_suite: { id: 99 } } }, { check: { head_sha: B } },
    { jobs: [job(), job({ id: 92 })] }, { check: { status: 'in_progress' } }]) {
    assert.equal((await inspectCI(options(), pr(), fixture(settings).api)).state, 'unverified');
  }
});

test('new run or rerun during collection invalidates prior success', async () => {
  for (const next of [run({ id: 71, run_number: 8 }), run({ run_attempt: 2 })]) {
    assert.equal((await inspectCI(options(), pr(), fixture({ runPages: [[run()], [next]] }).api)).state, 'pending');
  }
});

test('failure, cancellation, neutral, skipped and timed-out aggregate are not successful', async () => {
  for (const conclusion of ['failure', 'cancelled', 'neutral', 'skipped', 'timed_out']) {
    assert.equal((await inspectCI(options(), pr(), fixture({ jobs: [job({ conclusion })] }).api)).state, 'failed');
  }
  assert.equal((await inspectCI(options(), pr(), fixture({ jobs: [] }).api)).state, 'unverified');
});

test('actual checkout parents distinguish stale base, wrong head and missing proof', async () => {
  assert.equal((await inspectCI(options(), pr(), fixture({ commit: { sha: M, parents: [{ sha: C }, { sha: A }] } }).api)).state, 'stale_base');
  for (const settings of [{ log: 'no checkout proof' }, { commit: { sha: M, parents: [{ sha: B }, { sha: C }] } },
    { commit: { sha: M, parents: [{ sha: A }] } }])
    assert.equal((await inspectCI(options(), pr(), fixture(settings).api)).state, 'unverified');
  // Advertised PR metadata alone does not override actual checkout evidence.
  assert.equal((await inspectCI(options(), pr(), fixture({ runs: [run({ pull_requests: [{ number: 42, head: { sha: A }, base: { ref: 'main', sha: C } }] })] }).api)).state, 'success');
});

test('checkout parser accepts Unix/Windows commands and rejects multiple different checkouts', () => {
  assert.equal(checkoutRevision(checkoutLog(M)), M);
  assert.equal(checkoutRevision(checkoutLog(M).replace('/usr/bin/git', '"C:\\Program Files\\Git\\bin\\git.exe"')), M);
  assert.equal(checkoutRevision(checkoutLog(M) + checkoutLog(C)), null);
});

test('head/closure/conflict stop before evidence reads and invalidate a collected batch', async () => {
  for (const [extra, status] of [[{ head: { sha: C } }, 'head_changed'], [{ state: 'closed' }, 'pr_closed'],
    [{ mergeable: false }, 'merge_conflict']]) {
    for (const heads of [[pr(extra)], [pr(), pr(extra)]]) {
      const r = await inspectReview(options(), fixture({ heads, comments: [summary()] }).api);
      assert.equal(r.status, status);
    }
  }
});

test('base changes during or between polls are reported separately from head changes', async t => {
  assert.equal((await inspectReview(options(), fixture({ heads: [pr(), pr({ base: { ref: 'main', sha: C } })], comments: [summary()] }).api)).status, 'base_changed');
  const h = await harness(t);
  const api = fixture(() => ({ heads: [pr({ base: { ref: 'main', sha: h.now() > Date.parse(since) ? C : B } })] })).api;
  assert.equal((await waitForReview(h.opts, { ...h.deps, api })).status, 'base_changed');
});

test('draft, blocked and unknown mergeability are independent merge blockers, not conflicts', async () => {
  for (const extra of [{ draft: true }, { mergeable_state: 'blocked' }, { mergeable: null }]) {
    const r = await inspectReview(options({ waitCi: true }), fixture({ heads: [pr(extra)], comments: [summary()] }).api);
    assert.equal(r.status, 'merge_blocked');
    assert.equal(r.review.state, 'clean');
    assert.equal(r.ci.state, 'success');
  }
});
// Polling and bounded automatic recovery.
test('configured normal poll interval is not clamped to the error-backoff cap', async t => {
  const h = await harness(t, { intervalMs: 600000, timeoutMs: 1250000 });
  assert.equal((await waitForReview(h.opts, { ...h.deps, api: fixture().api })).status, 'timeout');
  assert.deepEqual(h.sleeps, [600000, 600000, 50000]);
});

test('stage progress only emits on changes and CI-only waiting does not wake the model', async t => {
  const h = await harness(t, { waitCi: true, timeoutMs: 4000 });
  const api = fixture(() => ({ comments: [summary({ status: h.now() < Date.parse(since) + 2000 ? 'running' : 'completed' })],
    runs: [run({ status: 'in_progress' })] })).api;
  const r = await waitForReview(h.opts, { ...h.deps, api });
  assert.equal(r.status, 'timeout');
  assert.equal(r.waiting_on, 'ci');
  assert.equal(h.messages.length, 2);
});

test('no review expires after exactly two rounds and only one POST', async t => {
  const h = await harness(t, { autoRetry: true, timeoutMs: 2000 });
  const r = await waitForReview(h.opts, { ...h.deps, api: fixture().api });
  assert.equal(r.status, 'timeout');
  assert.equal(h.posts.length, 1);
  assert.equal(h.now() - Date.parse(since), 4000);
  assert.equal(r.retry.trigger.id, 100);
  assert.equal(JSON.parse(await readFile(h.opts.retryState, 'utf8')).status, 'sent');
});

test('retry uses server cursor/time, excludes old receipt and accepts only the new round', async t => {
  const h = await harness(t, { autoRetry: true, timeoutMs: 3000 });
  const api = fixture(() => ({ comments: h.posts.length ? [summary({ at: h.now() < Date.parse(since) + 4000 ? since : new Date(h.now()).toISOString() })] : [] })).api;
  assert.equal((await waitForReview(h.opts, { ...h.deps, api })).status, 'review_completed');
  assert.equal(h.posts.length, 1);
  assert.equal(h.now() - Date.parse(since), 4000);
});

test('restart retains retry allowance and remaining deadline', async t => {
  const h = await harness(t, { autoRetry: true, timeoutMs: 2000 });
  await waitForReview(h.opts, { ...h.deps, api: fixture().api });
  h.set(Date.parse(since) + 3000);
  assert.equal((await waitForReview(h.opts, { ...h.deps, api: fixture().api })).status, 'timeout');
  assert.equal(h.now() - Date.parse(since), 4000);
  assert.equal(h.posts.length, 1);
});

test('uncertain POST and cancellation retain intent; restarting never blindly reposts', async t => {
  for (const cancel of [false, true]) {
    const h = await harness(t, { autoRetry: true, timeoutMs: 1000 });
    const control = new AbortController();
    let sends = 0;
    const postRetry = async () => { sends++; if (cancel) control.abort(); throw new Error('send interrupted'); };
    const wait = () => waitForReview(h.opts, { ...h.deps, api: fixture().api, postRetry, signal: control.signal });
    if (cancel) await assert.rejects(wait());
    else assert.equal((await wait()).status, 'retry_uncertain');
    const second = await waitForReview(h.opts, { ...h.deps, api: fixture().api, postRetry });
    assert.equal(second.status, 'retry_uncertain');
    assert.equal(sends, 1);
  }
});

test('concurrent claim, corrupted identity and result/journal overlap do not send', async t => {
  const h = await harness(t, { autoRetry: true, timeoutMs: 1000 });
  const claim = { version: 1, repo: 'acme/githux', pr: 42, head: A, status: 'intent' };
  const sleep = async ms => { await h.deps.sleep(ms); await saveResult(h.opts.retryState, claim); };
  assert.equal((await waitForReview(h.opts, { ...h.deps, sleep, api: fixture().api })).status, 'retry_uncertain');
  assert.equal(h.posts.length, 0);
  await saveResult(h.opts.retryState, { ...claim, head: B });
  await assert.rejects(waitForReview(h.opts, h.deps), /identity/);
  await assert.rejects(waitForReview({ ...h.opts, out: h.opts.retryState }, h.deps), /overwrite/);
});

test('single read, opt-out and custom-reviewer waits never create retry state', async t => {
  for (const extra of [{ timeoutMs: 0 }, { autoRetry: false }, { reviewers: ['custom-bot'] }]) {
    const h = await harness(t, { autoRetry: true, ...extra });
    assert.equal((await waitForReview(h.opts, { ...h.deps, api: fixture().api })).status, 'timeout');
    assert.equal(h.posts.length, 0);
    await assert.rejects(readFile(h.opts.retryState), { code: 'ENOENT' });
  }
});

test('CI-only, historical and incomplete-evidence waits never trigger another review', async t => {
  const cases = [
    [{ comments: [summary()], runs: [run({ status: 'in_progress' })] }, 'timeout'],
    [{ comments: [receipt()], inline: [comment({ commit_id: B, pull_request_review_id: 8 })] }, 'history_pending'],
    [{ comments: [summary({ findings: true })] }, 'evidence_incomplete'],
  ];
  for (const [settings, status] of cases) {
    const h = await harness(t, { autoRetry: true, timeoutMs: 3000, waitCi: true });
    assert.equal((await waitForReview(h.opts, { ...h.deps, api: fixture(settings).api })).status, status);
    assert.equal(h.posts.length, 0);
  }
});

test('deadline recheck catches newly completed review, changed head, closure or conflict', async t => {
  for (const status of ['review_completed', 'head_changed', 'pr_closed', 'merge_conflict']) {
    const h = await harness(t, { autoRetry: true, timeoutMs: 1000 });
    const api = fixture(() => {
      const expired = h.now() >= Date.parse(since) + 1000;
      const changes = { head_changed: { head: { sha: C } }, pr_closed: { state: 'closed' }, merge_conflict: { mergeable: false } };
      return { heads: [pr(expired ? changes[status] : {})], comments: expired && status === 'review_completed' ? [summary()] : [] };
    }).api;
    assert.equal((await waitForReview(h.opts, { ...h.deps, api })).status, status);
    assert.equal(h.posts.length, 0);
  }
});

test('known review failure, quota/environment messages and a newer trigger return without retry', async t => {
  for (const [comments, status] of [[[summary({ status: 'failed' })], 'review_failed'],
    [[receipt({ body: 'Environment is unavailable' })], 'unverified_bot_message'],
    [[{ id: 101, created_at: stamp, body: '@codex review', user: { login: 'owner' } }], 'trigger_changed']]) {
    const h = await harness(t, { autoRetry: true });
    assert.equal((await waitForReview(h.opts, { ...h.deps, api: fixture({ comments }).api })).status, status);
    assert.equal(h.posts.length, 0);
  }
});

test('server retry deadline overrides backoff and prevents deadline-time POST', async t => {
  const h = await harness(t, { autoRetry: true, timeoutMs: 60000 });
  let calls = 0;
  const api = async () => { calls++; throw Object.assign(new Error('HTTP 429'), { transient: true, retryAt: Date.parse(since) + 180000 }); };
  const r = await waitForReview(h.opts, { ...h.deps, api });
  assert.equal(r.status, 'timeout');
  assert.equal(calls, 1);
  assert.equal(h.posts.length, 0);
});

test('transient failures back off; fatal errors and external cancellation do not pretend success', async t => {
  const h = await harness(t);
  const successful = fixture({ comments: [summary()] }).api;
  let first = true;
  const api = async (...args) => { if (first) { first = false; throw Object.assign(new Error('503'), { transient: true }); } return successful(...args); };
  assert.equal((await waitForReview(h.opts, { ...h.deps, api })).status, 'review_completed');
  assert.deepEqual(h.sleeps, [2000]);
  await assert.rejects(waitForReview(h.opts, { api: async () => { throw new Error('401'); } }), /401/);
  const controller = new AbortController(); controller.abort();
  await assert.rejects(waitForReview(h.opts, { signal: controller.signal }), /abort/i);
});

// Real CLI adapter behavior, without network or credentials.
const http = (data, status = 200, headers = {}) => 'HTTP/2 ' + status + ' response\r\n' +
  Object.entries(headers).map(([key, value]) => key + ': ' + value + '\r\n').join('') + '\r\n' + (typeof data === 'string' ? data : JSON.stringify(data));

test('adapter uses serial GET argv and preserves every paginated page with conditional 304 reads', async () => {
  const calls = [];
  const api = createGithubApi({ execute: async (file, args, config) => {
    assert.equal(file, 'gh'); assert.equal(config.shell, false);
    assert.ok(args.includes('--include'));
    const prior = calls.filter(c => c[1] === args[1]).length; calls.push(args);
    if (prior) { assert.ok(args.includes('If-None-Match: v1')); return { stdout: http('', 304) }; }
    const second = args[1].includes('page=2');
    return { stdout: http(second ? [2] : [1], 200, { etag: 'v1', ...(second ? {} : { link: '<https://api.github.com/repos/acme/githux/items?page=2>; rel="next"' }) }) };
  } });
  for (let i = 0; i < 2; i++) assert.deepEqual(await api('repos/acme/githux/items', { paginate: true }), [1, 2]);
  assert.equal(calls.length, 4);
});

test('adapter honors Retry-After and primary reset headers; permissions remain fatal', async () => {
  for (const [status, headers, retryAt, transient] of [[429, { 'Retry-After': '180' }, 181000, true],
    [403, { 'X-RateLimit-Remaining': '0', 'X-RateLimit-Reset': '900' }, 900000, true],
    [403, {}, 0, false]]) {
    const api = createGithubApi({ now: () => 1000, execute: async () => { throw Object.assign(new Error('gh failed'), { stdout: http('{}', status, headers) }); } });
    await assert.rejects(api('repos/acme/githux/items'), e => e.retryAt === retryAt && e.transient === transient);
  }
});

test('adapter rejects bad JSON, pagination loops/foreign hosts and GraphQL partial errors/mutations', async () => {
  await assert.rejects(createGithubApi({ execute: async () => ({ stdout: http('not json') }) })('repos/acme/githux/items'), /JSON/);
  const api = createGithubApi({ execute: async () => ({ stdout: http([], 200, { link: '<https://evil.example/items>; rel="next"' }) }) });
  await assert.rejects(api('repos/acme/githux/items', { paginate: true }), /pagination/);
  await assert.rejects(api('graphql', { query: 'mutation { bad }' }), /read-only/);
  const partial = createGithubApi({ execute: async () => ({ stdout: http({ data: {}, errors: [{ message: 'forbidden' }] }) }) });
  await assert.rejects(partial('graphql', { query: 'query { viewer { login } }' }), /GraphQL/);
});

test('adapter text logs and immutable commit evidence can be cached without replaying commands', async () => {
  let reads = 0;
  const api = createGithubApi({ execute: async () => { reads++; return { stdout: http('raw log text') }; } });
  assert.equal(await api('repos/acme/githux/actions/jobs/91/logs', { text: true, immutable: true }), 'raw log text');
  assert.equal(await api('repos/acme/githux/actions/jobs/91/logs', { text: true, immutable: true }), 'raw log text');
  assert.equal(reads, 1);
});

test('POST sends only exact review request and accepts only valid server ID/time', async () => {
  const execute = async (_file, args, config) => {
    assert.deepEqual(args, ['api', 'repos/acme/githux/issues/42/comments', '--hostname', 'github.com', '--method', 'POST', '-f', 'body=@codex review']);
    assert.equal(config.shell, false);
    return { stdout: JSON.stringify({ id: 100, created_at: stamp, body: '@codex review' }) };
  };
  assert.equal((await postReviewRetry(options(), { execute })).id, 100);
  await assert.rejects(postReviewRetry(options(), { execute: async () => ({ stdout: '{}' }) }), /receipt/);
});

// Every CLI invocation owns an output run ID; stale success is invalidated.
async function quietMain(argv, deps) {
  const write = process.stdout.write;
  let output = '';
  process.stdout.write = chunk => { output += chunk; return true; };
  try { return { code: await main(argv, deps), output: JSON.parse(output) }; }
  finally { process.stdout.write = write; }
}

test('CLI replaces old success with running before querying and persists runtime errors with new run ID', async t => {
  const h = await harness(t); const out = join(h.dir, 'result.json');
  await saveResult(out, { run_id: 'old', status: 'checks_ready' });
  let running;
  const r = await quietMain([...args, '--out', out], { wait: async () => {
    running = JSON.parse(await readFile(out, 'utf8'));
    throw new Error('HTTP 401');
  } });
  const final = JSON.parse(await readFile(out, 'utf8'));
  assert.equal(running.status, 'running');
  assert.notEqual(running.run_id, 'old');
  assert.equal(final.status, 'error');
  assert.equal(final.run_id, running.run_id);
  assert.equal(r.code, 1);
});

test('CLI invalid arguments and cancellation overwrite old result instead of leaving success', async t => {
  const h = await harness(t); const out = join(h.dir, 'result.json');
  await saveResult(out, { status: 'checks_ready' });
  const invalid = await quietMain([...args, '--out', out, '--interval-seconds', '0']);
  assert.equal(invalid.code, 1);
  assert.equal(JSON.parse(await readFile(out, 'utf8')).status, 'error');
  const controller = new AbortController();
  const cancelled = await quietMain([...args, '--out', out], { signal: controller.signal, wait: async () => {
    controller.abort(); throw new Error('cancelled');
  } });
  assert.equal(cancelled.code, 130);
  assert.equal(JSON.parse(await readFile(out, 'utf8')).status, 'interrupted');
});

test('CLI will not use the retry journal as result output, even on parse errors', async t => {
  const h = await harness(t);
  await writeFile(h.opts.retryState, 'preserve');
  const r = await quietMain([...args, '--out', h.opts.retryState, '--retry-state', h.opts.retryState, '--unknown', 'true']);
  assert.equal(r.code, 1);
  assert.equal(await readFile(h.opts.retryState, 'utf8'), 'preserve');
});

test('result files replace atomically and have no leftover temporary files', async t => {
  const h = await harness(t); const out = join(h.dir, 'result.json');
  await saveResult(out, { status: 'running' }); await saveResult(out, { status: 'timeout' });
  assert.equal(JSON.parse(await readFile(out, 'utf8')).status, 'timeout');
});
test('inline feedback cannot be hidden by lagging thread API; explicitly resolved inline is not a new conflict', async () => {
  const inline = [comment()];
  const missing = fixture({ comments: [summary()], reviews: [review()], reviewInline: inline });
  assert.equal((await inspectReview(options(), missing.api)).status, 'evidence_conflict');
  const resolved = fixture({ comments: [summary()], reviews: [review()], inline: [comment({ resolved: true })] });
  assert.equal((await inspectReview(options(), resolved.api)).status, 'review_completed');
});

test('human current threads remain a separate blocker without rewriting Codex clean conclusion', async () => {
  const r = await inspectReview(options(), fixture({ comments: [receipt()], inline: [comment({ user: { login: 'human' }, pull_request_review_id: 999 })] }).api);
  assert.equal(r.review.state, 'clean');
  assert.equal(r.status, 'history_pending');
  assert.ok(r.next_actions.includes('verify_review_threads'));
});

test('Draft or blocked transition during final read prevents checks_ready', async () => {
  for (const extra of [{ draft: true }, { mergeable_state: 'blocked' }]) {
    assert.equal((await inspectReview(options({ waitCi: true }), fixture({ comments: [summary()], heads: [pr(), pr(extra)] }).api)).status, 'merge_blocked');
  }
});

test('a newer trigger invalidates previously clean or feedback evidence', async () => {
  const trigger = { id: 99, user: { login: 'owner' }, created_at: stamp, body: '@codex review' };
  for (const settings of [{ comments: [summary(), trigger] }, { comments: [trigger], reviews: [review({ body: 'Old batch' })] }])
    assert.equal((await inspectReview(options(), fixture(settings).api)).status, 'trigger_changed');
});

test('conditional pagination follows updated Link even when page content is unchanged', async () => {
  let count = 0;
  const api = createGithubApi({ execute: async (_file, args) => {
    count++;
    if (count === 1) return { stdout: http([1], 200, { etag: 'v1' }) };
    if (count === 2) return { stdout: http('', 304, { link: '<https://api.github.com/repos/acme/githux/items?page=2>; rel="next"' }) };
    assert.ok(args[1].includes('page=2'));
    return { stdout: http([2]) };
  } });
  assert.deepEqual(await api('repos/acme/githux/items', { paginate: true }), [1]);
  assert.deepEqual(await api('repos/acme/githux/items', { paginate: true }), [1, 2]);
});

test('in-flight requests are cancelled by the overall deadline', async () => {
  let aborted = false;
  const api = (_path, { signal }) => new Promise((_, reject) => signal.addEventListener('abort', () => {
    aborted = true; reject(new Error('aborted'));
  }, { once: true }));
  assert.equal((await waitForReview(options({ timeoutMs: 20 }), { api })).status, 'timeout');
  assert.equal(aborted, true);
});
test('stale PR base metadata cannot stand in for the current target branch', async () => {
  const r = await inspectReview(options({ waitCi: true }), fixture({ comments: [summary()], targets: [C] }).api);
  assert.equal(r.status, 'base_changed');
  assert.equal(r.integration.pr_base_sha, B);
  assert.equal(r.integration.base_sha, C);
  assert.equal(r.ci.tested_base, B);
});

test('text-log argv is accepted by the installed gh CLI without version-specific flags', async () => {
  const api = createGithubApi({ execute: async (file, args, config) => {
    assert.deepEqual(args, ['api', 'repos/acme/githux/actions/jobs/91/logs', '--hostname', 'github.com', '--method', 'GET', '--include']);
    // --help exercises gh's real flag parser without network or authentication.
    const help = execFileSync(file, [...args, '--help'], { ...config, stdio: 'pipe' });
    assert.match(help, /--include/);
    return { stdout: http(checkoutLog(M)) };
  } });
  assert.equal(checkoutRevision(await api('repos/acme/githux/actions/jobs/91/logs', { text: true })), M);
});

test('headerless EOF and upstream failures back off and recover; invalid CLI flags remain fatal', async t => {
  for (const [detail, transient] of [['Get "https://api.github.com/repos/acme/githux": EOF', true],
    ['HTTP 502: Bad Gateway', true], ['unknown flag: --bad-option', false]]) {
    const broken = createGithubApi({ execute: async () => { throw Object.assign(new Error('gh failed'), { stderr: detail }); } });
    await assert.rejects(broken('repos/acme/githux'), e => e.transient === transient);
    if (!transient) continue;
    const h = await harness(t);
    const good = fixture({ comments: [summary()] }).api;
    let first = true;
    const api = async (...args) => { if (first) { first = false; return broken(...args); } return good(...args); };
    assert.equal((await waitForReview(h.opts, { ...h.deps, api })).status, 'review_completed');
    assert.deepEqual(h.sleeps, [2000]);
    assert.equal(h.posts.length, 0);
  }
});


test('new gh escape gate retries a captured text GET once only when explicitly supported', async () => {
  const notice = 'the response contains terminal escape sequences; pass --allow-escape-sequences to output it anyway';
  for (const failTwice of [false, true]) {
    const calls = [];
    const api = createGithubApi({ execute: async (_file, args, config) => {
      calls.push([...args]);
      assert.equal(config.shell, false);
      if (calls.length === 1 || failTwice) throw Object.assign(new Error('gh failed'), { stdout: http('', 200), stderr: notice });
      return { stdout: http(checkoutLog(M)) };
    } });
    const read = api('repos/acme/githux/actions/jobs/91/logs', { text: true });
    if (failTwice) await assert.rejects(read, /terminal escape sequences/);
    else assert.equal(checkoutRevision(await read), M);
    assert.equal(calls.length, 2);
    assert.equal(calls[0].includes('--allow-escape-sequences'), false);
    assert.deepEqual(calls[1], [...calls[0], '--allow-escape-sequences']);
  }
  let calls = 0;
  const unsupported = createGithubApi({ execute: async () => { calls++; throw Object.assign(new Error('gh failed'), { stderr: 'unknown flag: --include' }); } });
  await assert.rejects(unsupported('repos/acme/githux/actions/jobs/91/logs', { text: true }), /unknown flag/);
  assert.equal(calls, 1);
});
