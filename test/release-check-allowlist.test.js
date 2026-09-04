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
const {
  expectedTarballPaths,
  diffTarballPaths,
  validateLatestChangelogHeading,
  systemExecutablePath,
  powershellPath,
  cmdPath,
} = require('../scripts/release-check.js');

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

test('release-check changelog: accepts an optional Unreleased entry and a real date', () => {
  assert.deepEqual(
    validateLatestChangelogHeading(
      '# Changelog\n\n## [Unreleased]\n\n## [1.2.3] - 2024-02-29\n',
      '1.2.3'
    ),
    { ok: true, version: '1.2.3', date: '2024-02-29', line: 5 }
  );
});

test('release-check changelog: rejects a heading that omits the date', () => {
  const result = validateLatestChangelogHeading('## [1.2.3]\n', '1.2.3');
  assert.equal(result.ok, false);
  assert.match(result.error, /must exactly match/);
});

test('release-check changelog: explains when no release heading exists', () => {
  const result = validateLatestChangelogHeading('# Changelog\n\n## [Unreleased]\n', '1.2.3');
  assert.equal(result.ok, false);
  assert.match(result.error, /no release version heading found/);
});

test('release-check changelog: rejects impossible calendar dates without JS rollover', () => {
  for (const date of ['0000-01-01', '2023-02-29', '2024-04-31', '2024-13-01', '2024-00-10']) {
    const result = validateLatestChangelogHeading(`## [1.2.3] - ${date}\n`, '1.2.3');
    assert.equal(result.ok, false, date);
    assert.match(result.error, /impossible calendar date/, date);
  }
});

test('release-check fixed Windows dependencies resolve under an absolute SystemRoot', () => {
  const env = { SystemRoot: 'D:\\Windows' };
  assert.equal(
    systemExecutablePath('cmd.exe', env),
    'D:\\Windows\\System32\\cmd.exe'
  );
  assert.equal(
    powershellPath(env),
    'D:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe'
  );
  assert.equal(cmdPath(env), 'D:\\Windows\\System32\\cmd.exe');
});

test('release-check fixed Windows dependencies honor an injected absolute SystemDirectory', () => {
  assert.equal(
    systemExecutablePath('cmd.exe', { SystemDirectory: 'E:\\Windows\\SysWOW64' }),
    'E:\\Windows\\SysWOW64\\cmd.exe'
  );
});

test('release-check fixed Windows dependencies reject relative system paths', () => {
  assert.throws(
    () => systemExecutablePath('cmd.exe', { SystemRoot: 'Windows' }),
    /SystemRoot\/SystemDirectory must be an absolute Windows path/
  );
  assert.throws(
    () => systemExecutablePath('cmd.exe', { SystemDirectory: 'System32' }),
    /SystemRoot\/SystemDirectory must be an absolute Windows path/
  );
});

test('release-check smoke sites use fixed system paths for PowerShell and cmd.exe', () => {
  const source = require('node:fs').readFileSync(
    path.join(repoRoot, 'scripts', 'release-check.js'),
    'utf8'
  );
  assert.match(source, /execFileSync\(\s*powershellPath\(\)/);
  assert.match(source, /execFileSync\(cmdPath\(\)/);
  assert.match(source, /cmdPath\(\), '\/c', 'exit', '7'/);
  assert.doesNotMatch(source, /execFileSync\(\s*['"]powershell(?:\.exe)?['"]/);
  assert.doesNotMatch(source, /execFileSync\(\s*['"]cmd\.exe['"]/);
});

test('release-check changelog: reports a version mismatch separately', () => {
  const result = validateLatestChangelogHeading('## [1.2.2] - 2024-01-31\n', '1.2.3');
  assert.deepEqual(result, {
    ok: false,
    error: 'latest heading is [1.2.2], but package.json version is 1.2.3',
  });
});
