import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

import {
  NOTICE_PATHS,
  LEGACY_FONT_LICENSE_PATHS,
  stageThirdPartyNotices,
  verifyThirdPartyNotices,
} from './stage-third-party-notices.mjs';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../..');

function fixtureRepository(t) {
  const root = mkdtempSync(join(tmpdir(), 'githux-notices-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const repository = join(root, 'repository');
  const destination = join(root, 'artifact');
  for (const path of new Set([...NOTICE_PATHS, ...Object.values(LEGACY_FONT_LICENSE_PATHS)])) {
    const source = join(repository, ...path.split('/'));
    cpSync(join(repoRoot, ...path.split('/')), source, { recursive: false });
  }
  return { repository, destination };
}

test('staging preserves every canonical notice and both legacy font paths byte for byte', t => {
  const { repository, destination } = fixtureRepository(t);
  stageThirdPartyNotices(repository, destination);
  verifyThirdPartyNotices(repository, destination, 'Linux');
  for (const path of NOTICE_PATHS) {
    assert.deepEqual(readFileSync(join(destination, ...path.split('/'))), readFileSync(join(repository, ...path.split('/'))), path);
  }
  assert.deepEqual(
    readFileSync(join(destination, 'OFL-NotoSansSC.txt')),
    readFileSync(join(repository, 'assets/fonts/githux-ui/OFL-NotoSansSC.txt')),
  );
  assert.deepEqual(
    readFileSync(join(destination, 'OFL-GoogleSansCode.txt')),
    readFileSync(join(repository, 'assets/fonts/google-sans-code/OFL.txt')),
  );
});

test('verification fails on a missing notice and on modified local text', t => {
  const { repository, destination } = fixtureRepository(t);
  stageThirdPartyNotices(repository, destination);
  const index = join(destination, 'THIRD_PARTY_NOTICES.md');
  rmSync(index);
  assert.throws(() => verifyThirdPartyNotices(repository, destination, 'Linux'), /Missing staged notice.*THIRD_PARTY_NOTICES/);

  stageThirdPartyNotices(repository, destination);
  const fontLicense = join(destination, 'assets/fonts/githux-ui/OFL-NotoSansSC.txt');
  writeFileSync(fontLicense, 'not the upstream license\n');
  assert.throws(() => verifyThirdPartyNotices(repository, destination, 'Linux'), /differs from assets\/fonts\/githux-ui\/OFL-NotoSansSC.txt/);
});

test('non-Windows verification checks real content without requiring SDK files', t => {
  const { repository, destination } = fixtureRepository(t);
  for (const platform of ['Linux', 'macOS']) {
    stageThirdPartyNotices(repository, destination);
    verifyThirdPartyNotices(repository, destination, platform);
    const runtimeFile = join(destination, 'microsoft.windowsappsdk.foundation.LICENSE.txt');
    writeFileSync(runtimeFile, 'not applicable on this platform\n');
    verifyThirdPartyNotices(repository, destination, platform);
    rmSync(runtimeFile);
  }
});

test('Windows verification requires the activation manifest and both package licenses', t => {
  const { repository, destination } = fixtureRepository(t);
  stageThirdPartyNotices(repository, destination);
  assert.throws(() => verifyThirdPartyNotices(repository, destination, 'Windows'), /Missing Windows runtime file.*githux-wasdk.manifest/);

  for (const path of [
    'githux-wasdk.manifest',
    'microsoft.windowsappsdk.foundation.LICENSE.txt',
    'microsoft.windowsappsdk.interactiveexperiences.LICENSE.txt',
  ]) writeFileSync(join(destination, path), `fixture ${path}\n`);
  verifyThirdPartyNotices(repository, destination, 'Windows');

  const runtimeLicense = join(destination, 'microsoft.windowsappsdk.foundation.LICENSE.txt');
  rmSync(runtimeLicense);
  assert.throws(() => verifyThirdPartyNotices(repository, destination, 'Windows'), /Missing Windows runtime file.*microsoft.windowsappsdk.foundation.LICENSE.txt/);
});

test('the CLI reports bad arguments and never follows a missing source', t => {
  const { destination } = fixtureRepository(t);
  const script = join(repoRoot, '.github/scripts/stage-third-party-notices.mjs');
  assert.throws(
    () => execFileSync(process.execPath, [script, 'stage', '--destination'], { encoding: 'utf8', stdio: 'pipe' }),
    error => error.status === 1 && error.stderr.includes('Missing value for --destination'),
  );
  const missingRepository = join(destination, 'missing repository');
  assert.throws(() => stageThirdPartyNotices(missingRepository, destination), /Missing repository notice index/);
});

test('a symlinked notice is not silently accepted as regular packaged content', t => {
  const { repository, destination } = fixtureRepository(t);
  stageThirdPartyNotices(repository, destination);
  rmSync(join(destination, 'docs/third-party-assets.md'));
  symlinkSync(join(repository, 'docs/third-party-assets.md'), join(destination, 'docs/third-party-assets.md'));
  assert.throws(() => verifyThirdPartyNotices(repository, destination, 'Linux'), /Expected staged notice to be a file/);
});
