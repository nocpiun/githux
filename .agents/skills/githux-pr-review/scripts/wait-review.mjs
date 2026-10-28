#!/usr/bin/env node
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { setTimeout as delay } from 'node:timers/promises';
import { mkdir, writeFile, rename, unlink, readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { randomUUID } from 'node:crypto';

import { ghApi, createGithubApi } from './github-api.mjs';
import { inspectReview, event, inspectHead } from './review-state.mjs';
export { ghApi, createGithubApi, inspectReview };
const exec = promisify(execFile);
const BOT = 'chatgpt-codex-connector[bot]';
const SHA = /^[0-9a-f]{40}$/i;
const ISO = /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:\d\d)$/;
const codes = { review_arrived: 0, error: 1, timeout: 2, head_changed: 3,
  pr_closed: 4, unverified_bot_message: 5, ci_failed: 6, review_failed: 7, merge_conflict: 8,
  review_completed: 0, checks_ready: 0, retry_uncertain: 9, history_pending: 10, evidence_incomplete: 11, evidence_conflict: 12,
  base_changed: 13, ci_unverified: 14, merge_blocked: 15, trigger_changed: 16, interrupted: 130 };
export const HELP = [
  '通过 gh CLI 等待 Codex 审查；超时自动重试一次，不判定通过。',
  'node wait-review.mjs --repo OWNER/REPO --pr N --head 完整40位SHA',
  '  [--since ISO时间] [--after-review-id N] [--after-comment-id N]',
  '  [--reviewer LOGIN ...] [--interval-seconds 30] [--timeout-seconds 3600]',
  '  [--wait-ci true|false] [--check NAME ...] [--evidence-grace-seconds 120]',
  '  [--auto-retry true|false] [--retry-state 文件] [--out 文件]',
  'timeout=0 只读查询一次；默认每轮1小时，最多自动重试一次。默认作者：' + BOT,
  '退出码：0 审查到达/完成或检查齐备；1 错误；2 超时；3 head改变；4 PR关闭；',
  '5 Bot消息需核实；6 CI失败；7 审查失败；8 合并冲突；9 重试发送状态不明；',
  '10 历史线程待处理；11 证据缺失；12 证据冲突；13 基线变化；14 CI待核实；',
  '15 合并阻塞；16 新触发轮次；130 中断。退出0不等于允许合并。',
  'blocked 只禁止合并，不停止审查/CI轮询或既有有界重审。',
].join('\n');

export function parseArgs(argv) {
  if (argv.length === 1 && argv[0] === '--help') return { help: true };
  const values = {};
  const allowed = new Set(['repo', 'pr', 'head', 'since', 'after-review-id',
    'after-comment-id', 'reviewer', 'interval-seconds', 'timeout-seconds', 'out', 'wait-ci', 'check', 'auto-retry', 'retry-state', 'evidence-grace-seconds']);
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
    waitCi: values['wait-ci'] === 'true', checks: [...new Set(['CI', ...(values.check ?? [])])],
    repo: values.repo, pr: number('pr', undefined, 1), head: values.head.toLowerCase(),
    since: values.since, afterReviewId: number('after-review-id', 0, 0),
    afterCommentId: number('after-comment-id', 0, 0), reviewers,
    intervalMs: number('interval-seconds', 30, 1, 3600) * 1000,
    timeoutMs: number('timeout-seconds', 3600, 0, 86400) * 1000, out: values.out,
    evidenceGraceMs: number('evidence-grace-seconds', 120, 0, 300) * 1000,
  };
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
  const retryEnabled = options.autoRetry && options.timeoutMs > 0 &&
    options.reviewers.length === 1 && options.reviewers[0] === BOT;
  const file = retryEnabled ? retryFile(options) : null;
  if (file && options.out && pathKey(file) === pathKey(options.out)) throw new Error('--out must not overwrite --retry-state');
  let retry = file ? await loadRetry(file, options) : null;
  let current = { ...options }, deadline = now() + options.timeoutMs;
  let errors = 0, attempts = 0, lastError, lastProgress = '', pending;
  let nextQueryAt = 0, softResult, evidenceDeadline;
  const softStates = new Set(['evidence_incomplete', 'evidence_conflict', 'ci_unverified']);
  const finish = (status, extra = {}) => event(current, status, {
    ...pending, status, attempts, last_error: lastError ?? null,
    retry: retry ? { ...retry, file } : null, ...extra,
  });
  if (retry?.status === 'intent') return finish('retry_uncertain', {
    message: '已有重试发送意图但无确认回执；先核对 GitHub 评论，不自动重发。',
  });
  const adopt = trigger => {
    current = { ...current, since: trigger.created_at, afterCommentId: trigger.id };
    deadline = Date.parse(trigger.created_at) + options.timeoutMs;
    softResult = undefined; evidenceDeadline = undefined;
  };
  if (retry?.status === 'sent' && options.afterCommentId <= retry.trigger.id) adopt(retry.trigger);

  const query = async budget => {
    const request = new AbortController();
    const forwardAbort = () => request.abort(signal.reason);
    signal?.addEventListener('abort', forwardAbort, { once: true });
    const timer = setTimeout(() => request.abort(), budget);
    try {
      const found = await inspectReview(current, api, request.signal, value => {
        pending = value;
        current.expectedBase ??= value.integration?.base_sha;
      });
      errors = 0; lastError = undefined; nextQueryAt = 0;
      if (found && softStates.has(found.status) && options.timeoutMs > 0 && options.evidenceGraceMs > 0) {
        softResult = found;
        evidenceDeadline ??= Math.min(deadline, now() + options.evidenceGraceMs);
        return now() >= evidenceDeadline ? found : null;
      }
      if (!found) softResult = undefined;
      return found;
    } catch (error) {
      if (signal?.aborted) throw error;
      if (!request.signal.aborted && !error.transient) throw error;
      errors++;
      lastError = request.signal.aborted ? '查询达到等待时限' : error.message;
      nextQueryAt = Math.max(nextQueryAt, error.retryAt || 0);
      pending = error.snapshot ? { ...error.snapshot, waiting_on: 'api' } : undefined;
      return null;
    } finally {
      clearTimeout(timer);
      signal?.removeEventListener('abort', forwardAbort);
    }
  };
  for (;;) {
    signal?.throwIfAborted();
    if (softResult && now() >= evidenceDeadline)
      return finish(softResult.status, { ...softResult, propagation_grace_expired: true });
    if (attempts && now() >= deadline) {
      if (softResult) return finish(softResult.status, { ...softResult, propagation_grace_expired: true });
      if (!retryEnabled || retry || now() < nextQueryAt) return finish('timeout');
      const found = await query(30000);
      if (found) return finish(found.status, found);
      if (softResult) return finish(softResult.status, softResult);
      if (!pending || pending.review_received || pending.newer_trigger || nextQueryAt > now())
        return finish('timeout', { message: '已有审查证据、新触发或当前状态无法确认，不重复触发。' });
      const latest = await api('repos/' + current.repo + '/pulls/' + current.pr, { signal });
      const changed = inspectHead(current, latest);
      if (changed) return finish(changed.status, changed);
      const target = await api('repos/' + current.repo + '/git/ref/heads/' + encodeURIComponent(latest.base?.ref), { signal });
      if (current.expectedBase && target.object?.sha !== current.expectedBase)
        return finish('base_changed', { current_base: { ref: latest.base?.ref, sha: target.object?.sha } });
      signal?.throwIfAborted();
      retry = { version: 1, repo: options.repo.toLowerCase(), pr: options.pr, head: options.head,
        status: 'intent', claimed_at: new Date(now()).toISOString() };
      await mkdir(dirname(file), { recursive: true });
      try { await writeFile(file, JSON.stringify(retry, null, 2) + '\n', { encoding: 'utf8', flag: 'wx' }); }
      catch (error) {
        if (error.code !== 'EEXIST') throw error;
        return finish('retry_uncertain', { message: '另一等待器已占用重试记录；不重复发送。' });
      }
      try {
        const trigger = await postRetry(current, { signal });
        validateTrigger({ ...trigger, body: '@codex review' });
        retry = { ...retry, status: 'sent', trigger };
        await saveResult(file, retry);
        adopt(trigger);
      } catch (error) {
        if (signal?.aborted) throw error;
        return finish('retry_uncertain', { message: '发送或记录状态不明，请核对 GitHub：' + error.message });
      }
      pending = undefined;
      progress('已自动发送一次 @codex review：' + retry.trigger.id + '；等待新轮次，不再自动重试。');
      continue;
    }
    attempts++;
    const end = Math.min(deadline, softResult ? evidenceDeadline : Infinity);
    const found = await query(options.timeoutMs === 0 ? 30000 : Math.max(1, end - now()));
    if (found) return finish(found.status, found);
    if (!options.timeoutMs) return finish('timeout');
    const phase = [pending?.review?.state ?? 'unknown', pending?.threads?.state ?? 'unknown', pending?.ci?.state ?? 'unknown'];
    const blocker = pending?.integration?.blockers?.includes('blocked_label') ? '；合并受 blocked 标签限制' : '';
    const message = lastError ? '查询退避：' + lastError : '等待：审查=' + phase[0] + '；线程=' + phase[1] + '；CI=' + phase[2] + blocker;
    if (message !== lastProgress) { progress(message); lastProgress = message; }
    const interval = errors ? Math.max(options.intervalMs, Math.min(120000, options.intervalMs * 2 ** Math.min(errors, 3))) : options.intervalMs;
    const remaining = Math.max(0, Math.min(deadline, softResult ? evidenceDeadline : Infinity) - now());
    await sleep(Math.min(Math.max(interval, nextQueryAt - now()), remaining), signal);
  }
}

function pathKey(value) {
  const absolute = resolve(value);
  return process.platform === 'win32' ? absolute.toLowerCase() : absolute;
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

function outputPath(argv) {
  const value = name => {
    const indexes = argv.flatMap((v, i) => v === name ? [i] : []);
    return indexes.length === 1 && argv[indexes[0] + 1] && !argv[indexes[0] + 1].startsWith('--') ? argv[indexes[0] + 1] : null;
  };
  const out = value('--out');
  if (!out) return null;
  const retry = value('--retry-state');
  const repo = value('--repo'), pr = value('--pr'), head = value('--head');
  const defaultRetry = repo && pr && head ? retryFile({ repo, pr, head: head.toLowerCase() }) : null;
  if ([retry, defaultRetry].some(path => path && pathKey(path) === pathKey(out)))
    throw new Error('--out must not overwrite the retry journal');
  return out;
}

export async function main(argv = process.argv.slice(2), { wait = waitForReview, signal } = {}) {
  const control = new AbortController();
  const stop = () => control.abort();
  process.once('SIGINT', stop); process.once('SIGTERM', stop);
  signal?.addEventListener('abort', stop, { once: true });
  if (signal?.aborted) stop();
  const identity = { schema_version: 2, run_id: randomUUID(), started_at: new Date().toISOString(),
    review_passed: null, verdict: 'not_evaluated' };
  let out, options;
  const complete = async result => {
    const final = { ...identity, ...result, finished_at: new Date().toISOString() };
    if (out) {
      try { await saveResult(out, final); }
      catch (error) { final.output_error = error.message; final.status = 'error'; }
    }
    process.stdout.write(JSON.stringify(final, null, 2) + '\n');
    return codes[final.status] ?? 1;
  };
  try {
    out = outputPath(argv);
    options = parseArgs(argv);
    if (options.help) { process.stdout.write(HELP + '\n'); return 0; }
    // Invalidate previous success before the first network request.
    if (out) await saveResult(out, { ...identity, repo: options.repo, pr: options.pr, head: options.head, status: 'running' });
    const result = await wait(options, { signal: control.signal, progress: text => process.stderr.write(text + '\n') });
    return await complete(result);
  } catch (error) {
    return await complete({ ...(error.snapshot ?? {}), status: control.signal.aborted ? 'interrupted' : 'error',
      repo: options?.repo, pr: options?.pr, head: options?.head, message: error.message });
  } finally {
    process.removeListener('SIGINT', stop); process.removeListener('SIGTERM', stop);
    signal?.removeEventListener('abort', stop);
  }
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href)
  process.exitCode = await main();
