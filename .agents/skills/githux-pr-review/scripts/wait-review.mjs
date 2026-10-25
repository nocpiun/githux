#!/usr/bin/env node
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { setTimeout as delay } from 'node:timers/promises';
import { mkdir, writeFile, rename, unlink, readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { randomUUID } from 'node:crypto';

const exec = promisify(execFile);
const BOT = 'chatgpt-codex-connector[bot]';
const SHA = /^[0-9a-f]{40}$/i;
const ISO = /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:\d\d)$/;
const codes = { review_arrived: 0, error: 1, timeout: 2, head_changed: 3,
  pr_closed: 4, unverified_bot_message: 5, ci_failed: 6, review_failed: 7, merge_conflict: 8,
  review_completed: 0, checks_ready: 0, retry_uncertain: 9, interrupted: 130 };
export const HELP = [
  '通过 gh CLI 等待 Codex 审查；超时自动重试一次，不判定通过。',
  'node wait-review.mjs --repo OWNER/REPO --pr N --head 完整40位SHA',
  '  [--since ISO时间] [--after-review-id N] [--after-comment-id N]',
  '  [--reviewer LOGIN ...] [--interval-seconds 30] [--timeout-seconds 3600]',
  '  [--wait-ci true|false] [--check NAME ...]',
  '  [--auto-retry true|false] [--retry-state 文件] [--out 文件]',
  'timeout=0 只读查询一次；默认每轮1小时，最多自动重试一次。默认作者：' + BOT,
  '退出码：0 审查到达/完成或检查齐备；1 错误；2 超时；3 head改变；4 PR关闭；',
  '5 Bot消息需核实；6 CI失败；7 审查失败；8 合并冲突；9 重试发送状态不明；130 中断。退出0不等于允许合并。',
].join('\n');

export function parseArgs(argv) {
  if (argv.length === 1 && argv[0] === '--help') return { help: true };
  const values = {};
  const allowed = new Set(['repo', 'pr', 'head', 'since', 'after-review-id',
    'after-comment-id', 'reviewer', 'interval-seconds', 'timeout-seconds', 'out', 'wait-ci', 'check', 'auto-retry', 'retry-state']);
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i].replace(/^--/, '');
    const value = argv[i + 1];
    if (!argv[i].startsWith('--') || !allowed.has(key) || !value || value.startsWith('--'))
      throw new Error('无效参数：' + argv[i]);
    if (key === 'reviewer' || key === 'check') (values[key] ??= []).push(value);
    else {
      if (Object.hasOwn(values, key)) throw new Error('重复参数：' + key);
      values[key] = value;
    }
  }
  if (!/^[A-Za-z0-9][A-Za-z0-9-]*\/[A-Za-z0-9_.-]+$/.test(values.repo ?? '') ||
      ['.', '..'].includes(values.repo?.split('/')[1])) throw new Error('需要 OWNER/REPO');
  if (!SHA.test(values.head ?? '')) throw new Error('需要完整40位 head SHA');
  const number = (key, fallback, min, max = Number.MAX_SAFE_INTEGER) => {
    const raw = values[key] ?? String(fallback);
    if (!/^\d+$/.test(raw) || !Number.isSafeInteger(Number(raw)) ||
        Number(raw) < min || Number(raw) > max) throw new Error('无效数值：' + key);
    return Number(raw);
  };
  if (values.since && (!ISO.test(values.since) || !Number.isFinite(Date.parse(values.since))))
    throw new Error('since 必须是带时区的 ISO 时间');
  const reviewers = values.reviewer ?? [BOT];
  if (reviewers.some(v => !/^[A-Za-z0-9][A-Za-z0-9-]*(?:\[bot\])?$/.test(v)))
    throw new Error('无效 reviewer login');
  if (number('after-comment-id', 0, 0) && !values.since)
    throw new Error('--after-comment-id requires --since from the trigger comment');
  if (values['wait-ci'] !== undefined && !['true', 'false'].includes(values['wait-ci']))
    throw new Error('wait-ci must be true or false');
  if (values.check && values['wait-ci'] !== 'true') throw new Error('--check requires --wait-ci true');
  if (values['auto-retry'] !== undefined && !['true', 'false'].includes(values['auto-retry']))
    throw new Error('auto-retry must be true or false');
  return {
    autoRetry: values['auto-retry'] !== 'false', retryState: values['retry-state'],
    waitCi: values['wait-ci'] === 'true', checks: [...new Set(values.check ?? ['CI'])],
    repo: values.repo, pr: number('pr', undefined, 1), head: values.head.toLowerCase(),
    since: values.since, afterReviewId: number('after-review-id', 0, 0),
    afterCommentId: number('after-comment-id', 0, 0), reviewers,
    intervalMs: number('interval-seconds', 30, 1, 3600) * 1000,
    timeoutMs: number('timeout-seconds', 3600, 0, 86400) * 1000, out: values.out,
  };
}

export async function ghApi(endpoint, { paginate = false, collection, query, signal, execute = exec } = {}) {
  if (query && (endpoint !== 'graphql' || !/^query\b/.test(query)))
    throw new Error('Only read-only GraphQL queries are supported');
  const args = ['api', endpoint, '--hostname', 'github.com', '--method', query ? 'POST' : 'GET'];
  if (query) args.push('-f', 'query=' + query);
  if (paginate) args.push('--paginate', '--slurp');
  let stdout;
  try {
    ({ stdout } = await execute('gh', args, {
      shell: false, windowsHide: true, encoding: 'utf8', timeout: 30000,
      maxBuffer: 32 * 1024 * 1024, signal,
      env: { ...process.env, GH_PROMPT_DISABLED: '1', GH_NO_UPDATE_NOTIFIER: '1', GH_PAGER: 'cat' },
    }));
  } catch (error) {
    if (signal?.aborted) throw error;
    const detail = String(error.stderr || error.message || 'gh failed').trim();
    const failure = new Error('gh 查询失败：' + detail);
    failure.transient = Boolean(error.killed || /HTTP (?:429|5\d\d)|rate limit|timed? ?out|ECONNRESET|ETIMEDOUT|TLS handshake|connection reset|dial tcp/i.test(detail));
    throw failure;
  }
  let data;
  try { data = JSON.parse(stdout); }
  catch { throw new Error('gh 返回了无效 JSON'); }
  if (query && data?.errors?.length) throw new Error('GraphQL query failed: ' + JSON.stringify(data.errors));
  if (paginate) {
    if (collection && Array.isArray(data)) data = data.map(page => page?.[collection]);
    if (!Array.isArray(data) || data.some(page => !Array.isArray(page)))
      throw new Error('gh 分页结果不是数组');
    return data.flat();
  }
  return data;
}

function event(options, status, extra = {}) {
  return { status, repo: options.repo, pr: options.pr, head: options.head,
    review_passed: null, verdict: 'not_evaluated', ...extra };
}

function inspectHead(options, pr) {
  if (!SHA.test(pr?.head?.sha ?? '') || !['open', 'closed'].includes(pr?.state))
    throw new Error('无效 PR 响应');
  if (pr.state !== 'open') return event(options, 'pr_closed', { merged: Boolean(pr.merged) });
  if (pr.head.sha.toLowerCase() !== options.head)
    return event(options, 'head_changed', { current_head: pr.head.sha });
  // REST mergeable is tri-state: null means GitHub is still computing it.
  // Protection, draft and failing checks are not merge conflicts.
  if (pr.mergeable === false) return event(options, 'merge_conflict', {
    mergeable: false, mergeable_state: pr.mergeable_state ?? null,
    base_ref: pr.base?.ref ?? null, base_sha: pr.base?.sha ?? null,
    message: 'PR 存在合并冲突，pull_request CI 无法启动。同步目标分支并解决冲突，推送后针对新 head 重新触发审查；继续轮询无法解决冲突。',
  });
  return null;
}


const CODEX_APP = 1144995;
const ACTIONS_APP = 15368;
const SUMMARY = '<!-- codex-pull-request-review-summary -->';
const RECEIPT = '<!-- codex-review-receipt:v1 -->';
function trustedCodex(comment) {
  return comment.user?.login === BOT && comment.user?.type === 'Bot' &&
    comment.performed_via_github_app?.id === CODEX_APP;
}
function receiptForHead(comment, head) {
  const match = /\*\*Reviewed commit:\*\*\s*`([a-f0-9]{10,40})`/i.exec(comment.body ?? '');
  return trustedCodex(comment) && comment.body?.includes(RECEIPT) &&
    match && head.startsWith(match[1].toLowerCase());
}

// This is the observed GitHub connector format, not an API stability promise.
// Security review edits must not refresh an old code-review round.
function parseSummary(comment, options, since) {
  if (!trustedCodex(comment)) return { invalid: 'Untrusted summary author/app' };
  const matches = [...(comment.body ?? '').matchAll(/<!-- codex-review-state:v1 (.*?) -->/g)];
  if (matches.length !== 1) return { invalid: 'Unknown or ambiguous summary schema' };
  let state;
  try { state = JSON.parse(matches[0][1]).code_review; } catch { /* invalid below */ }
  if (!state || !SHA.test(state.commit_sha ?? '') || typeof state.status !== 'string')
    return { invalid: 'Invalid code-review state' };
  if (state.commit_sha.toLowerCase() !== options.head) return { stale: true };
  const row = /^\|[^|\n]*\*\*Code Review\*\*\s*\|([^|\n]*)\|([^|\n]*)\|([^|\n]*)\|/m.exec(comment.body);
  const stamp = row && /datetime="([^"]+)"/.exec(row[1]);
  const time = stamp && Date.parse(stamp[1]);
  const updated = Date.parse(comment.updated_at);
  if (!Number.isFinite(time) || !Number.isFinite(updated))
    return { invalid: 'Missing code-review round timestamp' };
  if (time < since || updated < since) return { stale: true };
  if (time > updated + 1000 || (options.afterCommentId && row[3].trim() !== 'Manual request'))
    return { invalid: 'Cannot bind code-review round to the trigger' };
  if (!['running', 'completed', 'failed', 'cancelled', 'error'].includes(state.status))
    return { invalid: 'Unknown code-review status' };
  if (state.status === 'completed' && (typeof state.has_any_findings !== 'boolean' ||
      typeof state.has_unresolved_findings !== 'boolean' ||
      (!state.has_any_findings && state.has_unresolved_findings)))
    return { invalid: 'Incomplete or inconsistent findings flags' };
  return { evidence: {
    comment_id: comment.id, url: comment.html_url, updated_at: comment.updated_at,
    code_review_at: stamp[1], ...state,
  } };
}

async function inspectCI(options, api, signal) {
  const checks = await api('repos/' + options.repo + '/commits/' + options.head +
    '/check-runs?filter=latest&per_page=100', { paginate: true, collection: 'check_runs', signal });
  if (!Array.isArray(checks)) throw new Error('Invalid check-run list');
  const expected = options.checks ?? ['CI'];
  const records = [];
  let pending = false, failed = false;
  for (const name of expected) {
    const matching = checks.filter(c => c.name === name && c.app?.id === ACTIONS_APP &&
      c.head_sha?.toLowerCase() === options.head);
    if (matching.length > 1) throw new Error('Ambiguous GitHub Actions check: ' + name);
    if (!matching.length) { pending = true; records.push({ name, status: 'missing' }); continue; }
    const c = matching[0];
    records.push({ name, id: c.id, status: c.status, conclusion: c.conclusion, url: c.details_url });
    if (c.status !== 'completed') pending = true;
    else if (c.conclusion !== 'success') failed = true;
  }
  return { state: failed ? 'failed' : pending ? 'pending' : 'success', checks: records };
}

// REST inline comments do not expose thread resolution. Read every thread page,
// matching root node IDs back to REST IDs so replies inherit the root's status.
async function unresolvedCommentIds(options, inline, api, signal) {
  const [owner, name] = options.repo.split('/');
  const roots = new Set(), cursors = new Set();
  let cursor = null;
  for (;;) {
    const query = `query { repository(owner:${JSON.stringify(owner)}, name:${JSON.stringify(name)}) {
      pullRequest(number:${options.pr}) { reviewThreads(first:100, after:${JSON.stringify(cursor)}) {
        nodes { isResolved comments(first:1) { nodes { id } } }
        pageInfo { hasNextPage endCursor }
      } }
    } }`;
    const data = await api('graphql', { query, signal });
    const threads = data?.data?.repository?.pullRequest?.reviewThreads;
    if (!Array.isArray(threads?.nodes) || typeof threads.pageInfo?.hasNextPage !== 'boolean')
      throw new Error('Invalid review-thread response');
    for (const thread of threads.nodes) {
      const root = thread?.comments?.nodes?.[0]?.id;
      if (typeof thread?.isResolved !== 'boolean' || typeof root !== 'string')
        throw new Error('Missing review-thread resolution or root');
      if (!thread.isResolved) roots.add(root);
    }
    if (!threads.pageInfo.hasNextPage) break;
    cursor = threads.pageInfo.endCursor;
    if (typeof cursor !== 'string' || !cursor || cursors.has(cursor))
      throw new Error('Invalid review-thread pagination cursor');
    cursors.add(cursor);
  }
  const rootIds = new Set(inline.filter(c => roots.has(c.node_id)).map(c => c.id));
  return new Set(inline.filter(c => rootIds.has(c.id) || rootIds.has(c.in_reply_to_id)).map(c => c.id));
}

export async function inspectReview(options, api = ghApi, signal, observe = () => {}) {
  const base = 'repos/' + options.repo;
  const endpoint = base + '/pulls/' + options.pr;
  const get = path => api(path, { signal });
  const list = path => api(path + '?per_page=100', { paginate: true, signal });
  const pr = await get(endpoint);
  const change = inspectHead(options, pr);
  if (change) return change;
  const since = Date.parse(options.since ?? pr.created_at);
  if (!Number.isFinite(since)) throw new Error('无法确定审查时间边界');
  const authors = new Set(options.reviewers.map(v => v.toLowerCase()));
  const recent = (item, time, cursor) => authors.has(item.user?.login?.toLowerCase()) &&
    Number.isSafeInteger(item.id) && item.id > cursor &&
    Number.isFinite(Date.parse(time)) && Date.parse(time) >= since;
  const comments = await list(base + '/issues/' + options.pr + '/comments');
  if (!Array.isArray(comments)) throw new Error('无效普通评论列表');
  const pending = { waiting_on: 'review', review_received: false };
  const newerTrigger = comments.find(c => c.body?.trim() === '@codex review' &&
    Number.isSafeInteger(c.id) && c.id > options.afterCommentId && Date.parse(c.created_at) >= since);
  if (newerTrigger) pending.newer_trigger = { id: newerTrigger.id, created_at: newerTrigger.created_at };
  const summaries = [], unknown = [], receipts = [];
  for (const c of comments) {
    if (!authors.has(c.user?.login?.toLowerCase())) continue;
    if (c.body?.includes(SUMMARY)) {
      // The same comment ID is edited on subsequent heads; created_at/cursor do not identify this round.
      const parsed = parseSummary(c, options, since);
      if (parsed.evidence) summaries.push(parsed.evidence);
      else if (parsed.invalid && (recent(c, c.created_at, options.afterCommentId) ||
          Date.parse(c.updated_at) >= since)) unknown.push(c);
    } else if (recent(c, c.created_at, options.afterCommentId)) {
      if (receiptForHead(c, options.head)) receipts.push(c);
      else unknown.push(c);
    }
  }
  if (!summaries.length) unknown.push(...receipts);
  let result = null;
  if (summaries.length > 1 || unknown.length) {
    result = event(options, 'unverified_bot_message', {
      message: '无法唯一验证 Bot 状态；保留原始反馈供判断，不据此放行。',
      comments: unknown, summaries,
    });
  }
  const summary = summaries.length === 1 ? summaries[0] : null;
  if (summary) pending.summary = summary;
  if (summary?.status === 'completed' || receipts.length) {
    pending.review_received = true;
    pending.waiting_on = 'review_evidence';
    pending.receipts = receipts.map(c => ({ id: c.id, url: c.html_url, created_at: c.created_at }));
  }
  if (!result && summary) {
    if (['failed', 'cancelled', 'error'].includes(summary.status))
      result = event(options, 'review_failed', { summary });
    else if (summary.status === 'completed')
      // has_any_findings includes resolved history; only outstanding findings
      // require a formal batch. A clean rerun may have no new inline comments.
      result = event(options, summary.has_unresolved_findings ? 'review_arrived' : 'review_completed', { summary });
    // A known running summary owns batch completion, even if a formal review is already visible.
  }
  if ((!result && !summary) || result?.status === 'review_arrived') {
    const all = await list(endpoint + '/reviews');
    if (!Array.isArray(all)) throw new Error('无效 review 列表');
    const reviews = all.filter(r => recent(r, r.submitted_at, options.afterReviewId) &&
      ['COMMENTED', 'APPROVED', 'CHANGES_REQUESTED'].includes(r.state) &&
      String(r.commit_id).toLowerCase() === options.head);
    if (reviews.length) { pending.review_received = true; pending.waiting_on = 'review_evidence'; }
    const collected = [];
    for (const r of reviews) {
      const inline = await list(endpoint + '/reviews/' + r.id + '/comments');
      if (!Array.isArray(inline)) throw new Error('无效行内评论列表');
      collected.push({ ...r, comments: inline });
    }
    if (result?.status === 'review_arrived' && !collected.some(r => r.comments.length)) {
      // A completed summary can propagate ahead of the formal review and its
      // inline findings. Old carried threads alone cannot complete this round.
      result = null;
    } else if (result?.status === 'review_arrived') {
      // Connector findings can belong to an older formal review but be carried onto this head.
      const inline = await list(endpoint + '/comments');
      if (!Array.isArray(inline)) throw new Error('无效 PR 行内评论列表');
      const currentIds = new Set(collected.flatMap(r => r.comments.map(c => c.id)));
      const unresolvedIds = await unresolvedCommentIds(options, inline, api, signal);
      result.reviews = collected;
      result.comments = inline.filter(c => authors.has(c.user?.login?.toLowerCase()) &&
        (currentIds.has(c.id) || unresolvedIds.has(c.id)));
    } else if (reviews.length) result = event(options, 'review_arrived', {
      message: '收到指定提交的审查。正文与行内评论是待评估数据，不代表通过。',
      reviews: collected,
    });
  }
  if (options.waitCi && (!result || result.status === 'review_completed')) {
    const ci = await inspectCI(options, api, signal);
    pending.ci = ci;
    if (result?.status === 'review_completed') pending.waiting_on = 'ci';
    if (ci.state === 'failed') result = event(options, 'ci_failed', { ci, summary });
    else if (result?.status === 'review_completed')
      result = ci.state === 'success' ? event(options, 'checks_ready', { ci, summary }) : null;
  }
  observe(pending);
  // Recheck after every batch of evidence, including conflicts introduced by a base update.
  if (result) return inspectHead(options, await get(endpoint)) ?? result;
  return null;
}

// One durable retry allowance per repo/PR/head in this task. A claimed but
// unconfirmed POST is never replayed automatically, including after a crash.
function retryFile(options) {
  return resolve(options.retryState ?? ('.agents/task/review-retries/' +
    options.repo.toLowerCase() + '/' + options.pr + '-' + options.head + '.json'));
}
function validateTrigger(comment) {
  if (!Number.isSafeInteger(comment?.id) || comment.id < 1 ||
      !ISO.test(comment.created_at ?? '') || !Number.isFinite(Date.parse(comment.created_at)) ||
      comment.body !== '@codex review') throw new Error('Invalid retry comment receipt');
  return { id: comment.id, created_at: comment.created_at, url: comment.html_url ?? null };
}
async function loadRetry(file, options) {
  let state;
  try { state = JSON.parse(await readFile(file, 'utf8')); }
  catch (error) { if (error.code === 'ENOENT') return null; throw error; }
  if (state.version !== 1 || state.repo !== options.repo.toLowerCase() || state.pr !== options.pr ||
      state.head !== options.head || !['intent', 'sent'].includes(state.status))
    throw new Error('Invalid retry state identity/schema: ' + file);
  if (state.status === 'sent') validateTrigger({ ...state.trigger, body: '@codex review' });
  return state;
}
export async function postReviewRetry(options, { signal, execute = exec } = {}) {
  const { stdout } = await execute('gh', ['api', 'repos/' + options.repo + '/issues/' + options.pr +
    '/comments', '--hostname', 'github.com', '--method', 'POST', '-f', 'body=@codex review'], {
    shell: false, windowsHide: true, encoding: 'utf8', timeout: 30000, signal,
    maxBuffer: 1024 * 1024,
    env: { ...process.env, GH_PROMPT_DISABLED: '1', GH_NO_UPDATE_NOTIFIER: '1', GH_PAGER: 'cat' },
  });
  return validateTrigger(JSON.parse(stdout));
}

export async function waitForReview(options, {
  api = ghApi, now = Date.now, sleep = (ms, signal) => delay(ms, undefined, { signal }),
  signal, progress = () => {}, postRetry = postReviewRetry,
} = {}) {
  // Custom reviewer waits and diagnostic single reads must remain read-only.
  const retryEnabled = options.autoRetry && options.timeoutMs > 0 &&
    options.reviewers.length === 1 && options.reviewers[0] === BOT;
  const file = retryEnabled ? retryFile(options) : null;
  const pathKey = value => process.platform === 'win32' ? resolve(value).toLowerCase() : resolve(value);
  if (file && options.out && pathKey(file) === pathKey(options.out))
    throw new Error('--out must not overwrite --retry-state');
  let retry = file ? await loadRetry(file, options) : null;
  let current = { ...options }, deadline = now() + options.timeoutMs;
  let errors = 0, attempts = 0, lastError, lastProgress = '', pending;
  const finish = (status, extra = {}) => event(current, status, {
    attempts, last_error: lastError ?? null, ...pending,
    retry: retry ? { ...retry, file } : null, ...extra,
  });
  if (retry?.status === 'intent') return finish('retry_uncertain', {
    message: '已有重试发送意图但无确认回执；先核对 GitHub 评论，不自动重发。',
  });
  const adopt = trigger => {
    current = { ...options, since: trigger.created_at, afterCommentId: trigger.id };
    // Restarting the waiter does not grant a fresh timeout or retry allowance.
    deadline = Date.parse(trigger.created_at) + options.timeoutMs;
  };
  if (retry?.status === 'sent' && options.afterCommentId <= retry.trigger.id) adopt(retry.trigger);

  const query = async budget => {
    const request = new AbortController();
    const forwardAbort = () => request.abort(signal.reason);
    signal?.addEventListener('abort', forwardAbort, { once: true });
    const timer = setTimeout(() => request.abort(), budget);
    try {
      const found = await inspectReview(current, api, request.signal, value => { pending = value; });
      errors = 0; lastError = undefined;
      return found;
    } catch (error) {
      if (signal?.aborted) throw error;
      if (!request.signal.aborted && !error.transient) throw error;
      errors++;
      lastError = request.signal.aborted ? '查询达到等待时限' : error.message;
      pending = undefined; // Never use a stale snapshot to authorize a write.
      return null;
    } finally {
      clearTimeout(timer);
      signal?.removeEventListener('abort', forwardAbort);
    }
  };
  for (;;) {
    signal?.throwIfAborted();
    if (attempts && now() >= deadline) {
      if (!retryEnabled || retry) return finish('timeout');
      // A bounded fresh snapshot at expiry prevents retrying a review that has
      // just arrived, a CI-only wait, or a PR changed since the last poll.
      const found = await query(30000);
      if (found) return { ...found, retry };
      if (!pending || pending.review_received || pending.newer_trigger)
        return finish('timeout', { message: '未重触发：已有审查证据、新触发评论或无法确认当前状态。' });
      // Running summaries can lag a submitted formal batch.
      const submitted = await api('repos/' + current.repo + '/pulls/' + current.pr +
        '/reviews?per_page=100', { paginate: true, signal });
      if (!Array.isArray(submitted)) throw new Error('无效 review 列表');
      if (submitted.some(r => r.user?.login === BOT && r.commit_id?.toLowerCase() === current.head &&
          r.id > current.afterReviewId && Date.parse(r.submitted_at) >= Date.parse(current.since ?? '1970-01-01') &&
          ['COMMENTED', 'APPROVED', 'CHANGES_REQUESTED'].includes(r.state)))
        return finish('timeout', { waiting_on: 'review_evidence', review_received: true,
          message: '已有本轮正式审查，摘要尚未就绪；不重复触发。' });
      const changed = inspectHead(current, await api('repos/' + current.repo + '/pulls/' + current.pr, { signal }));
      if (changed) return changed;
      signal?.throwIfAborted();
      retry = { version: 1, repo: options.repo.toLowerCase(), pr: options.pr, head: options.head,
        status: 'intent', claimed_at: new Date(now()).toISOString() };
      await mkdir(dirname(file), { recursive: true });
      try {
        await writeFile(file, JSON.stringify(retry, null, 2) + '\n', { encoding: 'utf8', flag: 'wx' });
      } catch (error) {
        if (error.code !== 'EEXIST') throw error;
        return finish('retry_uncertain', { message: '另一等待器已占用重试记录；不重复发送。' });
      }
      try {
        const trigger = await postRetry(current, { signal });
        // Validate injected or future adapters as well as the real gh receipt.
        validateTrigger({ ...trigger, body: '@codex review' });
        retry = { ...retry, status: 'sent', trigger };
        await saveResult(file, retry);
        adopt(trigger);
      } catch (error) {
        if (signal?.aborted) throw error;
        return finish('retry_uncertain', { message: '重试发送或记录失败，请核对 GitHub 后恢复：' + error.message });
      }
      pending = undefined;
      progress('已自动发送一次 @codex review：' + retry.trigger.id + '；等待新轮次，不再自动重试。');
      continue;
    }
    attempts++;
    const found = await query(options.timeoutMs === 0 ? 30000 : Math.max(1, deadline - now()));
    if (found) return { ...found, retry: retry ? { ...retry, file } : null };
    if (!options.timeoutMs) return finish('timeout');
    const message = lastError ? '暂时无法查询，退避等待：' + lastError : '等待 ' + options.head + (options.waitCi ? ' 的 Codex 审查与 CI…' : ' 的 Codex 审查…');
    if (message !== lastProgress) { progress(message); lastProgress = message; }
    const interval = Math.min(120000, options.intervalMs * 2 ** Math.min(errors, 3));
    await sleep(Math.min(interval, Math.max(0, deadline - now())), signal);
  }
}

export async function saveResult(file, value) {
  const target = resolve(file);
  await mkdir(dirname(target), { recursive: true });
  const temporary = target + '.' + randomUUID() + '.tmp';
  try {
    await writeFile(temporary, JSON.stringify(value, null, 2) + '\n', { encoding: 'utf8', flag: 'wx' });
    await rename(temporary, target);
  } finally { await unlink(temporary).catch(error => { if (error.code !== 'ENOENT') throw error; }); }
}

export async function main(argv = process.argv.slice(2)) {
  const control = new AbortController();
  const stop = () => control.abort();
  process.once('SIGINT', stop);
  process.once('SIGTERM', stop);
  try {
    const options = parseArgs(argv);
    if (options.help) { process.stdout.write(HELP + '\n'); return 0; }
    const result = await waitForReview(options, {
      signal: control.signal, progress: text => process.stderr.write(text + '\n'),
    });
    if (options.out) await saveResult(options.out, result);
    process.stdout.write(JSON.stringify(result, null, 2) + '\n');
    return codes[result.status];
  } catch (error) {
    const status = control.signal.aborted ? 'interrupted' : 'error';
    process.stdout.write(JSON.stringify({ status, review_passed: null, verdict: 'not_evaluated',
      message: status === 'interrupted' ? '等待已中断' : error.message }) + '\n');
    return codes[status];
  } finally {
    process.removeListener('SIGINT', stop);
    process.removeListener('SIGTERM', stop);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href)
  process.exitCode = await main();
