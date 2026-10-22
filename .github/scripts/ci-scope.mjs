import { execFileSync } from 'node:child_process';
import { appendFileSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

// Only known documentation/tooling paths bypass Rust. New application paths
// automatically receive full checks instead of depending on an allow-list.
export function classifyChanges(files) {
  let app = false, scripts = false;
  for (const file of files) {
    if (file.startsWith('.github/')) { app = true; scripts = true; }
    else if (file.startsWith('docs/') || /^[^/]+\.md$/i.test(file)) continue;
    else if (file.startsWith('.agents/skills/')) {
      if (!/\.md$/i.test(file)) scripts = true;
    } else app = true;
  }
  return { app, scripts };
}

function sha(value) {
  if (!/^[a-f0-9]{40}$/i.test(value ?? '') || /^0+$/.test(value))
    throw new Error('Missing or invalid comparison commit');
  return value;
}

export function changedFiles(eventName, event, cwd = process.cwd()) {
  let args;
  if (eventName === 'pull_request') {
    // Compare the whole PR, not just its most recent (possibly docs-only) push.
    const base = sha(event.pull_request?.base?.sha), head = sha(event.pull_request?.head?.sha);
    args = ['diff', '--no-renames', '--name-only', '-z', `${base}...${head}`, '--'];
  } else if (eventName === 'push') {
    const head = sha(event.after);
    args = /^0{40}$/.test(event.before ?? '')
      ? ['ls-tree', '-r', '--name-only', '-z', head]
      : ['diff', '--no-renames', '--name-only', '-z', sha(event.before), head, '--'];
  } else throw new Error('Unsupported CI event: ' + eventName);
  // NUL delimiters preserve whitespace; disabling rename detection includes
  // deleted/renamed source paths even when the destination is documentation.
  const output = execFileSync('git', args, { cwd, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024,
    stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true });
  return output.split('\0').filter(Boolean);
}

export function verifyJobs(needs) {
  if (needs?.changes?.result !== 'success') throw new Error('Change detection did not succeed');
  const { app, scripts } = needs.changes.outputs ?? {};
  if (![app, scripts].every(v => v === 'true' || v === 'false'))
    throw new Error('Change detection outputs are missing or invalid');
  const expected = { 'code-check': app, build: app, 'script-check': scripts };
  for (const [job, required] of Object.entries(expected)) {
    const wanted = required === 'true' ? 'success' : 'skipped';
    if (needs[job]?.result !== wanted)
      throw new Error(`${job}: expected ${wanted}, got ${needs[job]?.result ?? 'missing'}`);
  }
  if (Object.keys(needs).some(job => job !== 'changes' && !(job in expected)))
    throw new Error('An upstream job has no gate policy');
}

export function main(command, env = process.env) {
  if (command === 'detect') {
    const event = JSON.parse(readFileSync(env.GITHUB_EVENT_PATH, 'utf8'));
    const files = changedFiles(env.GITHUB_EVENT_NAME, event);
    const scope = classifyChanges(files);
    appendFileSync(env.GITHUB_OUTPUT, `app=${scope.app}\nscripts=${scope.scripts}\n`);
    console.log(`Changed files: ${files.length}; Rust: ${scope.app}; agent scripts: ${scope.scripts}`);
  } else if (command === 'verify') {
    const needs = JSON.parse(env.CI_NEEDS);
    verifyJobs(needs);
    console.log('CI passed: required jobs succeeded; only out-of-scope jobs were skipped.');
  } else throw new Error('Usage: node ci-scope.mjs detect|verify');
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try { main(process.argv[2]); }
  catch (error) { console.error(error.message); process.exitCode = 1; }
}
