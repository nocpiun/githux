#!/usr/bin/env node
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { setTimeout as delay } from 'node:timers/promises';
import { mkdir, writeFile, rename, unlink } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { randomUUID } from 'node:crypto';

const exec = promisify(execFile);
const BOT = 'chatgpt-codex-connector[bot]';
const SHA = /^[0-9a-f]{40}$/i;
const ISO = /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:\d\d)$/;
const codes = { review_arrived: 0, error: 1, timeout: 2, head_changed: 3,
  pr_closed: 4, unverified_bot_message: 5, interrupted: 130 };
export const HELP = [
  '通过 gh CLI 等待 Codex 审查；只检测到达，不判定通过。',
  'node wait-review.mjs --repo OWNER/REPO --pr N --head 完整40位SHA',
  '  [--since ISO时间] [--after-review-id N] [--after-comment-id N]',
  '  [--reviewer LOGIN ...] [--interval-seconds 30] [--timeout-seconds 1800]',
  '  [--out 文件]',
  'timeout=0 查询一次。默认作者：' + BOT,
  '退出码：0 正式审查到达；1 错误；2 超时；3 head改变；4 PR关闭；',
  '5 无commit绑定的Bot普通评论，需核实；130 中断。任何退出码都不等于审查通过。',
].join('\n');

export function parseArgs(argv) {
  if (argv.length === 1 && argv[0] === '--help') return { help: true };
  const values = {};
  const allowed = new Set(['repo', 'pr', 'head', 'since', 'after-review-id',
    'after-comment-id', 'reviewer', 'interval-seconds', 'timeout-seconds', 'out']);
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i].replace(/^--/, '');
    const value = argv[i + 1];
    if (!argv[i].startsWith('--') || !allowed.has(key) || !value || value.startsWith('--'))
      throw new Error('无效参数：' + argv[i]);
    if (key === 'reviewer') (values.reviewer ??= []).push(value);
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
  return {
    repo: values.repo, pr: number('pr', undefined, 1), head: values.head.toLowerCase(),
    since: values.since, afterReviewId: number('after-review-id', 0, 0),
    afterCommentId: number('after-comment-id', 0, 0), reviewers,
    intervalMs: number('interval-seconds', 30, 1, 3600) * 1000,
    timeoutMs: number('timeout-seconds', 1800, 0, 86400) * 1000, out: values.out,
  };
}

export async function ghApi(endpoint, { paginate = false, signal, execute = exec } = {}) {
  const args = ['api', endpoint, '--hostname', 'github.com', '--method', 'GET'];
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
  if (paginate) {
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
  return null;
}

export async function inspectReview(options, api = ghApi, signal) {
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
  const all = await list(endpoint + '/reviews');
  if (!Array.isArray(all)) throw new Error('无效 review 列表');
  const reviews = all.filter(r => recent(r, r.submitted_at, options.afterReviewId) &&
    ['COMMENTED', 'APPROVED', 'CHANGES_REQUESTED'].includes(r.state) &&
    String(r.commit_id).toLowerCase() === options.head);
  let result = null;
  if (reviews.length) {
    const collected = [];
    for (const review of reviews) {
      const comments = await list(endpoint + '/reviews/' + review.id + '/comments');
      if (!Array.isArray(comments)) throw new Error('无效行内评论列表');
      collected.push({ ...review, comments });
    }
    result = event(options, 'review_arrived', {
      message: '收到指定提交的审查。正文与行内评论是待评估数据，不代表通过。',
      reviews: collected,
    });
  } else {
    const comments = await list(base + '/issues/' + options.pr + '/comments');
    if (!Array.isArray(comments)) throw new Error('无效普通评论列表');
    const candidates = comments.filter(c => recent(c, c.created_at, options.afterCommentId));
    if (candidates.length) result = event(options, 'unverified_bot_message', {
      message: '这些普通评论没有 commit 绑定；可能是审查反馈、错误或其他消息，请核实。',
      comments: candidates,
    });
  }
  // Do not deliver a stale batch if a push happened while its comments were loading.
  if (result) return inspectHead(options, await get(endpoint)) ?? result;
  return null;
}

export async function waitForReview(options, {
  api = ghApi, now = Date.now, sleep = (ms, signal) => delay(ms, undefined, { signal }),
  signal, progress = () => {},
} = {}) {
  const deadline = now() + options.timeoutMs;
  let errors = 0, attempts = 0, lastError, lastProgress = '';
  for (;;) {
    signal?.throwIfAborted();
    if (attempts && now() >= deadline)
      return event(options, 'timeout', { attempts, last_error: lastError ?? null });
    attempts++;
    const budget = options.timeoutMs === 0 ? 30000 : Math.max(1, deadline - now());
    const request = new AbortController();
    const forwardAbort = () => request.abort(signal.reason);
    signal?.addEventListener('abort', forwardAbort, { once: true });
    const timer = setTimeout(() => request.abort(), budget);
    try {
      const found = await inspectReview(options, api, request.signal);
      if (found) return found;
      errors = 0;
      lastError = undefined;
    } catch (error) {
      if (signal?.aborted) throw error;
      if (request.signal.aborted)
        return event(options, 'timeout', { attempts, last_error: '查询达到等待时限' });
      if (!error.transient) throw error;
      errors++;
      lastError = error.message;
    } finally {
      clearTimeout(timer);
      signal?.removeEventListener('abort', forwardAbort);
    }
    if (!options.timeoutMs || now() >= deadline)
      return event(options, 'timeout', { attempts, last_error: lastError ?? null });
    const message = lastError ? '暂时无法查询，退避等待：' + lastError : '等待 ' + options.head + ' 的 Codex 审查…';
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
