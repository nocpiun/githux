import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { cpSync, existsSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, relative, resolve, sep } from 'node:path';
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
  assert.ok(NOTICE_PATHS.length === 20, 'the notice set stays explicit at 20 canonical paths');
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

// SHA-256 of the license texts as shipped in the locked crate archives. The archive digests
// below equal the Cargo.lock checksums; docs/third-party-assets.md records how each text was
// extracted, so a replaced or truncated local copy fails here.
const LOCKED_CRATE_TEXTS = Object.freeze({
  'assets/licenses/chrono-0.4.45-license.txt': {
    archive: 'https://static.crates.io/crates/chrono/chrono-0.4.45.crate',
    archiveSha256: '1aa79e62e7697b8e29b513a68abacf485adcd1fe8284a4316c5ae868e6633327',
    textSha256: '946c9835d8034d24404f8cfec5f4654cee5dad17e944afc3d06d742cf2882831',
  },
  'assets/licenses/sha1_smol-1.0.1-license.txt': {
    archive: 'https://static.crates.io/crates/sha1_smol/sha1_smol-1.0.1.crate',
    archiveSha256: 'bbfa15b3dddfee50a0fff136974b3e1bde555604ba463834a7eb7deb6417705d',
    textSha256: '282a21b0710441c364025461a447362daf18f5798a7b72ae2f801a2aa536da51',
  },
  'assets/licenses/fast_image_resize-5.1.4-license-apache.txt': {
    archive: 'https://static.crates.io/crates/fast_image_resize/fast_image_resize-5.1.4.crate',
    archiveSha256: 'd372ab3252d8f162d858d675a3d88a8c33ba24a6238837c50c8851911c7e89cd',
    textSha256: '5caa63b85f1c4683ed495f09991128fd777fda36066e2fcdf0f8533212776ed1',
  },
  'assets/licenses/fast_image_resize-5.1.4-license-mit.txt': {
    archive: 'https://static.crates.io/crates/fast_image_resize/fast_image_resize-5.1.4.crate',
    archiveSha256: 'd372ab3252d8f162d858d675a3d88a8c33ba24a6238837c50c8851911c7e89cd',
    textSha256: '59c7bec3a007a291de2369c504545cb61b5f4d7ac9893e50d523f2aff4fcc7c6',
  },
});

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

// Git may check licence texts out with CRLF on Windows (core.autocrlf), which changes
// nothing but the line terminators. Compare the LF-normalized text so the digests stay
// the archive's bytes on every checkout without weakening content verification.
function textSha256(bytes) {
  return sha256(Buffer.from(bytes.toString('latin1').replace(/\r\n/g, '\n'), 'latin1'));
}

test('chrono, sha1_smol and fast_image_resize texts are the locked archive bytes', () => {
  const lock = readFileSync(join(repoRoot, 'Cargo.lock'), 'utf8');
  const lockedChecksums = new Map();
  for (const block of lock.split('[[package]]').slice(1)) {
    const name = /^name = "([^"]+)"/m.exec(block)?.[1];
    const version = /^version = "([^"]+)"/m.exec(block)?.[1];
    const checksum = /^checksum = "([^"]+)"/m.exec(block)?.[1];
    if (name && version && checksum) lockedChecksums.set(`${name} ${version}`, checksum);
  }
  for (const [path, expected] of Object.entries(LOCKED_CRATE_TEXTS)) {
    assert.ok(NOTICE_PATHS.includes(path), `${path} must be staged with the artifacts`);
    assert.equal(textSha256(readFileSync(join(repoRoot, ...path.split('/')))), expected.textSha256, path);
    const locked = [...lockedChecksums].find(([key]) => path.includes(key.replace(' ', '-')));
    assert.ok(locked, `${path} must belong to a crate resolved in Cargo.lock`);
    assert.equal(locked[1], expected.archiveSha256, `${path} archive digest must match Cargo.lock`);
  }
});

test('every license link in the notice indexes resolves and is staged', () => {
  for (const index of ['THIRD_PARTY_NOTICES.md', 'docs/third-party-assets.md']) {
    const indexDir = dirname(join(repoRoot, ...index.split('/')));
    const markdown = readFileSync(join(repoRoot, ...index.split('/')), 'utf8');
    const targets = [...markdown.matchAll(/\]\(([^)\s]+)\)/g)]
      .map(match => match[1])
      .filter(target => !target.includes('://') && !target.startsWith('#'));
    for (const target of targets) {
      const path = target.split('#')[0];
      if (!path) continue;
      const absolute = resolve(indexDir, ...path.split('/'));
      assert.ok(existsSync(absolute), `${index} links to a missing file: ${target}`);
      const repositoryRelative = relative(repoRoot, absolute).split(sep).join('/');
      if (repositoryRelative.startsWith('assets/licenses/')) {
        assert.ok(NOTICE_PATHS.includes(repositoryRelative), `${index} links to an un-staged license: ${target}`);
      }
    }
  }
});

test('a symlinked notice is not silently accepted as regular packaged content', t => {
  const { repository, destination } = fixtureRepository(t);
  stageThirdPartyNotices(repository, destination);
  rmSync(join(destination, 'docs/third-party-assets.md'));
  symlinkSync(join(repository, 'docs/third-party-assets.md'), join(destination, 'docs/third-party-assets.md'));
  assert.throws(() => verifyThirdPartyNotices(repository, destination, 'Linux'), /Expected staged notice to be a file/);
});
