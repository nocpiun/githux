import { copyFileSync, lstatSync, mkdirSync, readFileSync } from 'node:fs';
import { dirname, isAbsolute, relative, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

// Keep the repository-relative layout: links in the two Markdown indexes then
// resolve to the same checked-in license texts inside a downloaded artifact.
export const NOTICE_PATHS = Object.freeze([
  'THIRD_PARTY_NOTICES.md',
  'docs/third-party-assets.md',
  'assets/fonts/githux-ui/OFL-NotoSansSC.txt',
  'assets/fonts/google-sans-code/OFL.txt',
  'assets/licenses/chrono-0.4.45-license.txt',
  'assets/licenses/ely-mit.txt',
  'assets/licenses/fast_image_resize-5.1.4-license-apache.txt',
  'assets/licenses/fast_image_resize-5.1.4-license-mit.txt',
  'assets/licenses/gpui-pre-apache-2.0.txt',
  'assets/licenses/ibm-plex-sans-ofl.txt',
  'assets/licenses/linguist-mit.txt',
  'assets/licenses/lucide-icons-isc.txt',
  'assets/licenses/pulldown-cmark-mit.txt',
  'assets/licenses/resvg-0.46.0-license-apache.txt',
  'assets/licenses/resvg-0.46.0-license-mit.txt',
  'assets/licenses/sha1_smol-1.0.1-license.txt',
  'assets/licenses/sqlite-public-domain.txt',
  'assets/licenses/tiny-skia-0.11.4-license.txt',
  'assets/licenses/usvg-0.46.0-license-apache.txt',
  'assets/licenses/usvg-0.46.0-license-mit.txt',
]);

// These were the only notice paths in older debug artifacts. Keep them alongside
// the canonical source-relative copies so adding the index does not rename them.
export const LEGACY_FONT_LICENSE_PATHS = Object.freeze({
  'OFL-NotoSansSC.txt': 'assets/fonts/githux-ui/OFL-NotoSansSC.txt',
  'OFL-GoogleSansCode.txt': 'assets/fonts/google-sans-code/OFL.txt',
});

export const WINDOWS_RUNTIME_FILES = Object.freeze([
  'githux-wasdk.manifest',
  'microsoft.windowsappsdk.foundation.LICENSE.txt',
  'microsoft.windowsappsdk.interactiveexperiences.LICENSE.txt',
]);

function resolveInside(root, path) {
  const result = resolve(root, ...path.split('/'));
  const fromRoot = relative(resolve(root), result);
  if (fromRoot === '..' || fromRoot.startsWith('../') || fromRoot.startsWith('..\\') || isAbsolute(fromRoot)) {
    throw new Error(`Notice path escapes its root: ${path}`);
  }
  return result;
}

function requireFile(path, description) {
  let stat;
  try { stat = lstatSync(path); }
  catch { throw new Error(`Missing ${description}: ${path}`); }
  if (!stat.isFile()) throw new Error(`Expected ${description} to be a file: ${path}`);
  return stat;
}

function copyNotice(repoRoot, destination, sourcePath, destinationPath = sourcePath) {
  const source = resolveInside(repoRoot, sourcePath);
  const target = resolveInside(destination, destinationPath);
  requireFile(source, 'source notice');
  const bytes = readFileSync(source);
  mkdirSync(dirname(target), { recursive: true });
  copyFileSync(source, target);
  if (!readFileSync(target).equals(bytes)) {
    throw new Error(`Staged notice differs from its source: ${destinationPath}`);
  }
}

export function stageThirdPartyNotices(repoRoot, destination) {
  requireFile(resolve(repoRoot, 'THIRD_PARTY_NOTICES.md'), 'repository notice index');
  mkdirSync(destination, { recursive: true });
  for (const path of NOTICE_PATHS) copyNotice(repoRoot, destination, path);
  for (const [target, source] of Object.entries(LEGACY_FONT_LICENSE_PATHS)) {
    copyNotice(repoRoot, destination, source, target);
  }
  console.log(`Staged ${NOTICE_PATHS.length + Object.keys(LEGACY_FONT_LICENSE_PATHS).length} third-party notice files.`);
}

export function verifyThirdPartyNotices(repoRoot, destination, runnerOs = process.env.RUNNER_OS ?? process.platform) {
  const expected = new Map(NOTICE_PATHS.map(path => [path, path]));
  for (const [target, source] of Object.entries(LEGACY_FONT_LICENSE_PATHS)) expected.set(target, source);
  for (const [target, source] of expected) {
    requireFile(resolveInside(repoRoot, source), 'source notice');
    const sourceBytes = readFileSync(resolveInside(repoRoot, source));
    const stagedPath = resolveInside(destination, target);
    requireFile(stagedPath, 'staged notice');
    const stagedBytes = readFileSync(stagedPath);
    if (!stagedBytes.equals(sourceBytes)) throw new Error(`Staged notice differs from ${source}: ${target}`);
  }
  const isWindows = runnerOs === 'Windows' || runnerOs === 'win32';
  const isNonWindows = ['Linux', 'linux', 'macOS', 'darwin'].includes(runnerOs);
  if (isWindows) {
    for (const path of WINDOWS_RUNTIME_FILES) {
      const runtimeFile = resolveInside(destination, path);
      if (requireFile(runtimeFile, 'Windows runtime file').size === 0) {
        throw new Error(`Windows runtime file is empty: ${path}`);
      }
    }
  } else if (!isNonWindows) {
    throw new Error(`Unknown runner OS for notice verification: ${runnerOs}`);
  }
  console.log(`Verified ${expected.size} notice files${isWindows ? ` and ${WINDOWS_RUNTIME_FILES.length} Windows runtime files` : ''}.`);
}

function parseArguments(argv) {
  const command = argv[0];
  if (command !== 'stage' && command !== 'verify') {
    throw new Error('Usage: node stage-third-party-notices.mjs stage|verify --destination <directory> [--platform <runner OS>]');
  }
  let destination, platform;
  for (let index = 1; index < argv.length; index++) {
    const flag = argv[index];
    if (flag !== '--destination' && flag !== '--platform') throw new Error(`Unknown argument: ${flag}`);
    const value = argv[++index];
    if (!value || value.startsWith('--')) throw new Error(`Missing value for ${flag}`);
    if (flag === '--destination') {
      if (destination !== undefined) throw new Error('Duplicate --destination');
      destination = value;
    } else {
      if (platform !== undefined) throw new Error('Duplicate --platform');
      platform = value;
    }
  }
  if (!destination) throw new Error('Missing --destination');
  return { command, destination: resolve(destination), platform };
}

const scriptPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : '';
if (scriptPath && import.meta.url === scriptPath) {
  try {
    const { command, destination, platform } = parseArguments(process.argv.slice(2));
    const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
    if (command === 'stage') stageThirdPartyNotices(repoRoot, destination);
    else verifyThirdPartyNotices(repoRoot, destination, platform);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
