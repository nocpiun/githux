const SHA = /^[a-f0-9]{40}$/i;
const positive = value => Number.isSafeInteger(value) && value > 0;

// Checkout's final git-log command identifies the revision actually checked out.
// Unknown log formats are evidence gaps, never inferred from the PR's current merge ref.
export function checkoutRevision(log) {
  const lines = String(log).replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '')
    .split(/\r?\n/).map(line => line.replace(/^\d{4}-\d\d-\d\dT\S+\s+/, ''));
  const revisions = [];
  for (let i = 0; i < lines.length - 1; i++) {
    if (/^\[command\].*[/\\]git(?:\.exe)?"? log -1 --format=%H\s*$/.test(lines[i]) && SHA.test(lines[i + 1].trim()))
      revisions.push(lines[i + 1].trim().toLowerCase());
  }
  const unique = [...new Set(revisions)];
  return unique.length === 1 ? unique[0] : null;
}

export async function inspectCI(options, pr, api, signal) {
  const base = 'repos/' + options.repo;
  const config = { signal };
  const list = (path, collection) => api(path, { signal, paginate: true, collection });
  const workflow = await api(base + '/actions/workflows/' + encodeURIComponent('ci.yml'), config);
  if (!positive(workflow?.id) || workflow.path !== '.github/workflows/ci.yml')
    return { state: 'unverified', reason: 'Unexpected CI workflow identity' };
  const endpoint = base + '/actions/workflows/' + workflow.id + '/runs?event=pull_request&head_sha=' + options.head + '&per_page=100';
  const select = runs => {
    if (!Array.isArray(runs)) throw new Error('Invalid workflow-run list');
    const matched = runs.filter(r => r.workflow_id === workflow.id && r.path === workflow.path &&
      r.repository?.full_name?.toLowerCase() === options.repo.toLowerCase() && r.event === 'pull_request' &&
      r.head_sha?.toLowerCase() === options.head && Array.isArray(r.pull_requests) &&
      r.pull_requests.some(p => p.number === options.pr && p.head?.sha?.toLowerCase() === options.head &&
        p.base?.ref === pr.base.ref));
    if (matched.some(r => !positive(r.id) || !positive(r.run_number) || !positive(r.run_attempt)))
      throw new Error('Invalid workflow-run identity');
    return matched.sort((a, b) => b.run_number - a.run_number || b.id - a.id || b.run_attempt - a.run_attempt)[0];
  };
  const run = select(await list(endpoint, 'workflow_runs'));
  if (!run) return { state: 'pending', reason: 'No CI run for this PR/head', checks: [] };
  const identity = { id: run.id, attempt: run.run_attempt, workflow_id: workflow.id,
    path: run.path, url: run.html_url, head: run.head_sha, event: run.event };
  const result = (state, more = {}) => ({ state, run: identity, ...more });
  const jobs = await list(base + '/actions/runs/' + run.id + '/attempts/' + run.run_attempt + '/jobs?per_page=100', 'jobs');
  if (jobs.some(j => j.run_id !== run.id || j.run_attempt !== run.run_attempt || j.head_sha?.toLowerCase() !== options.head))
    return result('unverified', { reason: 'Jobs do not belong to the selected run/attempt/head' });
  if (jobs.some(j => !positive(j.id))) return result('unverified', { reason: 'Invalid CI job ID' });
  const checks = [];
  for (const name of [...new Set(['CI', ...(options.checks ?? [])])]) {
    const matching = jobs.filter(j => j.name === name);
    if (matching.length > 1) return result('unverified', { reason: 'Ambiguous CI job: ' + name });
    if (!matching.length) { checks.push({ name, state: 'pending', reason: 'missing' }); continue; }
    const job = matching[0];
    const prefix = 'https://api.github.com/' + base + '/check-runs/';
    const id = job.check_run_url?.startsWith(prefix) ? job.check_run_url.slice(prefix.length) : '';
    if (!/^\d+$/.test(id)) return result('unverified', { reason: 'Invalid job check-run URL' });
    const check = await api(base + '/check-runs/' + id, config);
    if (check.id !== Number(id) || check.app?.id !== 15368 || check.head_sha?.toLowerCase() !== options.head ||
        check.name !== name || check.check_suite?.id !== run.check_suite_id)
      return result('unverified', { reason: 'Check identity does not match the selected Actions run' });
    if (check.status !== job.status || check.conclusion !== job.conclusion)
      return result('unverified', { reason: 'Job/check propagation disagreement' });
    const state = job.status !== 'completed' ? 'pending' : job.conclusion === 'success' ? 'success' : 'failed';
    checks.push({ name, id: check.id, job_id: job.id, state, conclusion: job.conclusion, url: job.html_url });
  }
  if (run.status === 'completed' && checks.some(c => c.reason === 'missing'))
    return result('unverified', { checks, reason: 'Completed CI run is missing a required job' });
  const failed = checks.some(c => c.state === 'failed') ||
    (run.status === 'completed' && run.conclusion !== 'success');
  let candidate;
  if (failed) candidate = result('failed', { checks });
  else if (run.status !== 'completed' || checks.some(c => c.state !== 'success')) candidate = result('pending', { checks });
  else {
    // Verify every executed successful job, including the aggregate, against
    // the actual merge parents. Metadata's advertised base is not proof.
    const executed = jobs.filter(j => j.conclusion !== 'skipped');
    const revisions = [];
    for (const job of executed) {
      if (job.status !== 'completed' || job.conclusion !== 'success')
        return result('unverified', { checks, reason: 'Unexpected non-successful upstream job' });
      let revision;
      try { revision = checkoutRevision(await api(base + '/actions/jobs/' + job.id + '/logs',
        { signal, text: true, immutable: true })); }
      catch (error) {
        if (signal?.aborted || error.transient) throw error;
        return result('unverified', { checks, reason: 'Cannot read checkout evidence: ' + error.message });
      }
      if (!revision) return result('unverified', { checks, reason: 'Missing or ambiguous checkout revision', job_id: job.id });
      revisions.push({ job_id: job.id, sha: revision });
    }
    const shas = [...new Set(revisions.map(r => r.sha))];
    if (shas.length !== 1) return result('unverified', { checks, revisions, reason: 'Jobs tested different revisions' });
    const commit = await api(base + '/commits/' + shas[0], { signal, immutable: true });
    if (commit.sha !== shas[0] || !Array.isArray(commit.parents) || commit.parents.length !== 2 ||
        commit.parents[1]?.sha !== options.head)
      return result('unverified', { checks, revisions, reason: 'Checkout is not a merge of the requested head' });
    const testedBase = commit.parents[0].sha;
    candidate = result(testedBase === pr.base.sha ? 'success' : 'stale_base', {
      checks, revisions, tested_base: testedBase, current_base: pr.base.sha, merge_sha: shas[0],
    });
  }
  // A rerun/new run that appeared while collecting evidence supersedes old success.
  const latest = select(await list(endpoint, 'workflow_runs'));
  if (!latest || latest.id !== run.id || latest.run_attempt !== run.run_attempt || latest.status !== run.status ||
      latest.conclusion !== run.conclusion)
    return result('pending', { checks, reason: 'CI run/attempt changed while reading evidence' });
  return candidate;
}