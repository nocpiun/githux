// Offline fixture. Never calls a model or opens a window.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
const args = process.argv.slice(2);
const option = key => args[args.indexOf(key) + 1];
const settings = JSON.parse(fs.readFileSync(path.join(path.dirname(fileURLToPath(import.meta.url)), 'fake-settings.json'), 'utf8'));
if (args.includes('--list-models')) {
  if (settings.inventoryFailure) { console.error('Fake inventory failure'); process.exit(7); }
  console.log('provider model context max-out thinking images');
  console.log(`${settings.provider} ${settings.model} 200k 32k yes no`);
  process.exit(0);
}
const dir = option('--session-dir');
let session;
if (args.includes('--session')) {
  session = option('--session');
  if (!fs.existsSync(session)) process.exit(9);
} else {
  const id = option('--session-id');
  session = path.join(dir, `fixture_${id}.jsonl`);
  fs.writeFileSync(session, `${JSON.stringify({ type: 'session', version: 3, id, timestamp: new Date().toISOString(), cwd: process.cwd() })}\n`, { flag: 'wx' });
}
fs.appendFileSync(path.join(dir, 'invocations.jsonl'), `${JSON.stringify({ args, cwd: process.cwd(), session })}\n`);
console.error('Fake interactive child: inherited stderr is visible.');
const gate = path.join(dir, 'exit-gate');
const timer = setInterval(() => {
  if (fs.existsSync(gate)) { clearInterval(timer); process.exit(settings.exitCode ?? 0); }
}, 50);
