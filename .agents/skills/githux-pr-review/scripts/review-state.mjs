import { ghApi } from './github-api.mjs';
import { inspectCI } from './ci-evidence.mjs';
const BOT = 'chatgpt-codex-connector[bot]';
const SHA = /^[a-f0-9]{40}$/i;
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
  // Owner labels are independent of GitHub's mergeable_state/protection settings.
  if (!Array.isArray(pr.labels) || pr.labels.some(label => typeof label?.name !== 'string'))
    throw new Error('Missing or invalid PR labels');
  // Labels gate final readiness, not review collection or timeout recovery.
  // REST mergeable is tri-state: null means GitHub is still computing it.
  // Protection, draft and failing checks are not merge conflicts.
  if (pr.mergeable === false) return event(options, 'merge_conflict', {
    mergeable: false, mergeable_state: pr.mergeable_state ?? null,
    base_ref: pr.base?.ref ?? null, base_sha: pr.base?.sha ?? null,
    message: 'PR 存在合并冲突，pull_request CI 无法启动。同步目标分支并解决冲突，推送后针对新 head 重新触发审查；继续轮询无法解决冲突。',
  });
  return null;
}


function mergeBlockers(pr) {
  return [pr.labels.some(label => label.name.toLowerCase() === 'blocked') && 'blocked_label',
    pr.draft && 'draft', pr.mergeable == null && 'mergeability_unknown',
    ['blocked', 'behind'].includes(pr.mergeable_state) && pr.mergeable_state].filter(Boolean);
}

const CODEX_APP = 1144995;
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


function commentEvidence(c) {
  const fields = ['id', 'node_id', 'body', 'created_at', 'updated_at', 'submitted_at', 'state',
    'commit_id', 'original_commit_id', 'pull_request_review_id', 'in_reply_to_id', 'path', 'line', 'start_line', 'side'];
  return { ...Object.fromEntries(fields.filter(k => c[k] !== undefined).map(k => [k, c[k]])),
    url: c.html_url ?? c.url, user: c.user ? { login: c.user.login, type: c.user.type, id: c.user.id } : null,
    app_id: c.performed_via_github_app?.id ?? null };
}

function receiptState(comment, head) {
  if (!receiptForHead(comment, head)) return 'unknown';
  const first = comment.body.replace(RECEIPT, '').trim().split(/\r?\n/)[0];
  return /^Codex Review: Didn't find any major issues\.(?:\s.*)?$/.test(first) ? 'clean' : 'unknown';
}

async function readThreads(options, inline, currentReviews, since, api, signal) {
  const [owner, name] = options.repo.split('/');
  const currentIds = new Set(currentReviews.map(r => r.id));
  const result = { state: 'clear', current: [], historical: [], resolved_comment_ids: [] };
  const visited = new Set();
  let cursor = null;
  for (;;) {
    const query = `query { repository(owner:${JSON.stringify(owner)}, name:${JSON.stringify(name)}) {
      pullRequest(number:${options.pr}) { reviewThreads(first:100, after:${JSON.stringify(cursor)}) {
        nodes { id isResolved isOutdated comments(first:1) { nodes { id } } }
        pageInfo { hasNextPage endCursor }
      } }
    } }`;
    const data = await api('graphql', { query, signal });
    const threads = data?.data?.repository?.pullRequest?.reviewThreads;
    if (!Array.isArray(threads?.nodes) || typeof threads.pageInfo?.hasNextPage !== 'boolean')
      throw new Error('Invalid review-thread response');
    for (const thread of threads.nodes) {
      const node = thread?.comments?.nodes?.[0]?.id;
      if (typeof thread?.isResolved !== 'boolean' || typeof node !== 'string')
        throw new Error('Missing review-thread resolution/root');
      const root = inline.find(c => c.node_id === node);
      if (thread.isResolved) {
        if (root) result.resolved_comment_ids.push(...inline.filter(c => c.id === root.id || c.in_reply_to_id === root.id).map(c => c.id));
        continue;
      }
      if (!root) {
        result.state = 'incomplete';
        (result.missing_roots ??= []).push(node);
        continue;
      }
      const current = currentIds.has(root.pull_request_review_id) ||
        (root.commit_id?.toLowerCase() === options.head && Date.parse(root.created_at) >= since);
      const entry = { id: thread.id, outdated: thread.isOutdated ?? null,
        // Keep human replies too: their context can explain a disputed finding.
        comments: inline.filter(c => c.id === root.id || c.in_reply_to_id === root.id) };
      result[current ? 'current' : 'historical'].push(entry);
    }
    if (!threads.pageInfo.hasNextPage) break;
    cursor = threads.pageInfo.endCursor;
    if (typeof cursor !== 'string' || !cursor || visited.has(cursor)) throw new Error('Invalid review-thread cursor');
    visited.add(cursor);
  }
  if (result.state !== 'incomplete' && (result.current.length || result.historical.length)) result.state = 'pending';
  return result;
}

function decide(options, snapshot) {
  const { review, threads, ci, integration } = snapshot;
  const actions = [];
  const ownerBlocked = integration.blockers.includes('blocked_label');
  if (ownerBlocked) actions.push('wait_for_owner_unblock');
  if (review.state === 'feedback') actions.push('assess_review');
  if (threads.historical.length) actions.push('verify_historical_threads');
  if (review.state === 'clean' && threads.current.length) actions.push('verify_review_threads');
  if (ci.state === 'failed') actions.push('fix_ci');
  if (ci.state === 'stale_base') actions.push('verify_current_base');
  let status = 'waiting';
  if (snapshot.newer_trigger) status = 'trigger_changed';
  else if (review.state === 'unverified') status = 'unverified_bot_message';
  else if (review.state === 'failed') status = 'review_failed';
  else if (review.state === 'feedback') status = 'review_arrived';
  else if (ci.state === 'failed') status = 'ci_failed';
  else if (review.state === 'conflict') status = 'evidence_conflict';
  else if (review.state === 'incomplete' || threads.state === 'incomplete') status = 'evidence_incomplete';
  else if (ci.state === 'stale_base') status = 'base_changed';
  else if (ci.state === 'unverified') status = 'ci_unverified';
  else if (review.state === 'clean' && threads.state === 'pending') status = 'history_pending';
  else if (review.state === 'clean') {
    if (!options.waitCi) status = ownerBlocked ? 'merge_blocked' : 'review_completed';
    else if (ci.state === 'success') status = integration.blockers.length ? 'merge_blocked' : 'checks_ready';
  }
  if (['unverified_bot_message', 'evidence_conflict', 'evidence_incomplete', 'ci_unverified', 'trigger_changed'].includes(status)) actions.push('verify_evidence');
  if (status === 'review_failed') actions.push('inspect_review_failure');
  if (status === 'merge_blocked') actions.push('verify_merge_blockers');
  if (status === 'checks_ready' || status === 'review_completed') actions.push('verify_final_merge_policy');
  if (status === 'waiting') actions.push('wait');
  return event(options, status, { ...snapshot, next_actions: [...new Set(actions)],
    waiting_on: review.state === 'running' || review.state === 'pending' ? 'review' :
      ['incomplete', 'conflict'].includes(review.state) ? 'review_evidence' : ci.state === 'pending' ? 'ci' : null,
    review_received: !['pending', 'running'].includes(review.state) || review.reviews.length > 0 || review.receipts.length > 0,
  });
}

export async function inspectReview(options, api = ghApi, signal, observe = () => {}) {
  const base = 'repos/' + options.repo, endpoint = base + '/pulls/' + options.pr;
  const get = path => api(path, { signal });
  const list = path => api(path + '?per_page=100', { signal, paginate: true });
  const pr = await get(endpoint);
  const changed = inspectHead(options, pr);
  if (changed) return changed;
  if (!SHA.test(pr.base?.sha ?? '') || typeof pr.base?.ref !== 'string') throw new Error('Missing PR base identity');
  const targetHead = async ref => {
    const target = await get(base + '/git/ref/heads/' + encodeURIComponent(ref));
    if (target.ref !== 'refs/heads/' + ref || target.object?.type !== 'commit' || !SHA.test(target.object?.sha ?? ''))
      throw new Error('Invalid target branch reference');
    return target.object.sha;
  };
  const advertisedBase = pr.base.sha;
  pr.base = { ...pr.base, sha: await targetHead(pr.base.ref) };
  const integration = { base_sha: pr.base.sha, pr_base_sha: advertisedBase, base_ref: pr.base.ref, mergeable: pr.mergeable,
    draft: Boolean(pr.draft), mergeable_state: pr.mergeable_state, labels: pr.labels.map(label => label.name),
    blockers: mergeBlockers(pr) };
  if (options.expectedBase && options.expectedBase !== pr.base.sha)
    return event(options, 'base_changed', { integration, previous_base: options.expectedBase, next_actions: ['verify_current_base'] });
  const since = Date.parse(options.since ?? pr.created_at);
  if (!Number.isFinite(since)) throw new Error('Missing review time boundary');
  const authors = new Set(options.reviewers.map(s => s.toLowerCase()));
  const recent = (item, at, cursor) => authors.has(item.user?.login?.toLowerCase()) &&
    Number.isSafeInteger(item.id) && item.id > cursor && Number.isFinite(Date.parse(at)) && Date.parse(at) >= since;
  const comments = await list(base + '/issues/' + options.pr + '/comments');
  if (!Array.isArray(comments)) throw new Error('Invalid issue comments');
  const review = { state: 'pending', summaries: [], receipts: [], reviews: [], unknown: [], reason: null };
  for (const c of comments) {
    if (!authors.has(c.user?.login?.toLowerCase())) continue;
    if (c.body?.includes(SUMMARY)) {
      const parsed = parseSummary(c, options, since);
      if (parsed.evidence) review.summaries.push(parsed.evidence);
      else if (parsed.invalid && (recent(c, c.created_at, options.afterCommentId) || Date.parse(c.updated_at) >= since)) review.unknown.push(c);
    } else if (recent(c, c.created_at, options.afterCommentId)) {
      if (receiptState(c, options.head) === 'clean') review.receipts.push(c);
      else review.unknown.push(c);
    }
  }
  const all = await list(endpoint + '/reviews');
  if (!Array.isArray(all)) throw new Error('Invalid formal reviews');
  for (const r of all.filter(r => recent(r, r.submitted_at, options.afterReviewId) &&
    ['COMMENTED', 'APPROVED', 'CHANGES_REQUESTED'].includes(r.state) && r.commit_id?.toLowerCase() === options.head)) {
    if (r.user.login === BOT && r.user.type !== 'Bot') { review.unknown.push(r); continue; }
    const inline = await list(endpoint + '/reviews/' + r.id + '/comments');
    if (!Array.isArray(inline)) throw new Error('Invalid inline review comments');
    review.reviews.push({ ...r, comments: inline });
  }
  const inline = await list(endpoint + '/comments');
  if (!Array.isArray(inline)) throw new Error('Invalid PR inline comments');
  const threads = await readThreads(options, inline, review.reviews, since, api, signal);
  const summary = review.summaries.length === 1 ? review.summaries[0] : null;
  const clean = review.receipts.length > 0 || (summary?.status === 'completed' && !summary.has_unresolved_findings);
  const feedback = review.reviews.some(r => r.state === 'CHANGES_REQUESTED' ||
    (!r.comments.length && Boolean(r.body?.trim())) ||
    r.comments.some(c => !threads.resolved_comment_ids.includes(c.id))) ||
    threads.current.some(t => authors.has(t.comments[0]?.user?.login?.toLowerCase()));
  if (review.unknown.length || review.summaries.length > 1) review.state = 'unverified';
  else if (['failed', 'cancelled', 'error'].includes(summary?.status)) review.state = 'failed';
  else if (clean && feedback) { review.state = 'conflict'; review.reason = 'Clean evidence conflicts with current feedback'; }
  else if (summary?.status === 'running') {
    review.state = review.receipts.length ? 'conflict' : 'running';
    review.reason = review.receipts.length ? 'Receipt arrived before summary completion' : null;
  } else if (clean) review.state = 'clean';
  else if (summary?.status === 'completed') {
    review.state = feedback ? 'feedback' : 'incomplete';
    review.reason = feedback ? null : 'Completed findings summary has no current feedback or clean receipt';
  } else if (review.reviews.length) review.state = 'feedback'; // Even body-only/empty COMMENTED is delivered for assessment.
  else if (threads.current.length) { review.state = 'incomplete'; review.reason = 'Current threads have no submitted batch'; }
  const newer = comments.filter(c => c.body?.trim() === '@codex review' && c.id > options.afterCommentId && Date.parse(c.created_at) >= since)
    .sort((a, b) => b.id - a.id)[0];
  review.receipts = review.receipts.map(commentEvidence);
  review.unknown = review.unknown.map(commentEvidence);
  review.reviews = review.reviews.map(r => ({ ...commentEvidence(r), comments: r.comments.map(commentEvidence) }));
  for (const thread of [...threads.current, ...threads.historical]) thread.comments = thread.comments.map(commentEvidence);
  const snapshot = { review, threads, ci: { state: 'not_requested' }, integration,
    newer_trigger: newer ? { id: newer.id, created_at: newer.created_at, url: newer.html_url } : null,
    discussion: comments.filter(c => !authors.has(c.user?.login?.toLowerCase()) &&
      (Date.parse(c.created_at) >= since || c.body?.includes('githux:arbitration-required:v1'))).map(commentEvidence),
  };
  try { if (options.waitCi) snapshot.ci = await inspectCI(options, pr, api, signal); }
  catch (error) { snapshot.ci = { state: 'unavailable', message: error.message }; error.snapshot = snapshot; throw error; }
  let last;
  try { last = await get(endpoint); } catch (error) { error.snapshot = snapshot; throw error; }
  const invalidated = inspectHead(options, last);
  if (invalidated) return invalidated;
  const finalBase = last.base?.ref === pr.base.ref ? await targetHead(last.base.ref) : null;
  if (finalBase !== pr.base.sha || last.base?.ref !== pr.base.ref)
    return event(options, 'base_changed', { ...snapshot, current_base: { ref: last.base?.ref, sha: finalBase }, next_actions: ['verify_current_base'] });
  integration.labels = last.labels.map(label => label.name);
  integration.mergeable = last.mergeable;
  integration.draft = Boolean(last.draft);
  integration.mergeable_state = last.mergeable_state;
  integration.blockers = mergeBlockers(last);
  const result = decide(options, snapshot);
  observe(result);
  return result.status === 'waiting' ? null : result;
}
export { event, inspectHead };
