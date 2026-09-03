'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const path = require('node:path');

// Regression tests for the round-15 review finding P3-1: release-check.js's
// packed-tarball check used to compare only the bin/ subset of the packed
// file list, so a stray file under install/ or skills/ (both shipped whole
// via package.json's `files` whitelist) landed in the real published tarball
// while the gate stayed green. The script exposes the pure allowlist and
// comparator without running the gate (top-level return on
// require.main !== module); here they are exercised against the actual
// `npm pack --dry-run --json` file list of this checkout.
const { expectedTarballPaths, diffTarballPaths } = require('../scripts/release-check.js');

const repoRoot = path.join(__dirname, '..');

// Same constraint scripts/release-check.js documents for spawning npm:
// execFileSync cannot launch npm.cmd on Windows without a shell (EINVAL).
// Under `npm test` npm_execpath is set, so run node against npm's own JS
// entrypoint; direct `node --test` runs have no npm_execpath and fall back
// to PATH npm: on Windows as a single shell command string (npm.cmd cannot
// be execFileSync'd directly, and an args array + shell:true trips Node's
// DEP0190 deprecation warning; the string is a fixed literal with no
// interpolation and cwd is passed via options, so the shell never sees
// anything attacker-influenced), elsewhere as a plain argv array.
let packed = null;
function packedTarballPaths() {
  if (!packed) {
    const out = process.env.npm_execpath
      ? execFileSync(process.execPath, [process.env.npm_execpath, 'pack', '--dry-run', '--json'], {
          cwd: repoRoot,
          encoding: 'utf8',
        })
      : process.platform === 'win32'
        ? execFileSync('npm pack --dry-run --json', {
            cwd: repoRoot,
            encoding: 'utf8',
            shell: true,
          })
        : execFileSync('npm', ['pack', '--dry-run', '--json'], {
            cwd: repoRoot,
            encoding: 'utf8',
          });
    const [info] = JSON.parse(out);
    packed = info.files.map((f) => f.path);
  }
  return packed;
}

test('release-check allowlist: this checkout packs exactly the 54 expected paths', () => {
  const diff = diffTarballPaths(packedTarballPaths());
  assert.deepEqual(diff, { missing: [], extra: [] });
});

test('release-check allowlist: stray install/ file is rejected and named', () => {
  const { missing, extra } = diffTarballPaths([
    ...packedTarballPaths(),
    'install/unexpected-review-probe.txt',
  ]);
  assert.deepEqual(missing, []);
  assert.deepEqual(extra, ['install/unexpected-review-probe.txt']);
});

test('release-check allowlist: stray skills/ file is rejected and named', () => {
  const { missing, extra } = diffTarballPaths([
    ...packedTarballPaths(),
    'skills/win-nice/unexpected-review-probe.txt',
  ]);
  assert.deepEqual(missing, []);
  assert.deepEqual(extra, ['skills/win-nice/unexpected-review-probe.txt']);
});

test('release-check allowlist: stray bin/ file is still rejected and named', () => {
  const { missing, extra } = diffTarballPaths([
    ...packedTarballPaths(),
    'bin/unexpected-review-probe.txt',
  ]);
  assert.deepEqual(missing, []);
  assert.deepEqual(extra, ['bin/unexpected-review-probe.txt']);
});

test('release-check allowlist: a missing expected path is rejected and named', () => {
  const { missing, extra } = diffTarballPaths(
    packedTarballPaths().filter((p) => p !== 'skills/win-nice/SKILL.md')
  );
  assert.deepEqual(extra, []);
  assert.deepEqual(missing, ['skills/win-nice/SKILL.md']);
});

test('release-check allowlist: 54 unique paths = 42 launchers + 12 non-bin literals', () => {
  assert.equal(expectedTarballPaths.length, 54);
  assert.equal(new Set(expectedTarballPaths).size, 54);
  assert.equal(expectedTarballPaths.filter((p) => p.startsWith('bin/')).length, 42);
  // Pinned on purpose: if this list changes, release-check.js's
  // expectedNonBinPaths and this test must change in the same commit.
  assert.deepEqual(
    expectedTarballPaths.filter((p) => !p.startsWith('bin/')).sort(),
    [
      'CHANGELOG.md',
      'LICENSE-APACHE',
      'LICENSE-MIT',
      'README.md',
      'package.json',
      'install/cli.js',
      'install/install.js',
      'install/manifest.js',
      'install/paths.js',
      'install/skill.js',
      'install/uninstall.js',
      'skills/win-nice/SKILL.md',
    ].sort()
  );
});
