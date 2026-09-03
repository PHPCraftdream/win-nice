#!/usr/bin/env node
'use strict';
// Builds the actual npm tarball, installs it into an isolated WIN_NICE_HOME
// (never the real one - never touches the real PATH/registry either), and
// verifies what a real `npm install -g win-nice` would produce. Source-tree
// tests (npm test, Pester) never touch the packed artifact itself, so a
// files-whitelist mistake or a stale CHANGELOG/tag mismatch can slip past
// both suites - this is what publish.yml runs right before `npm publish`.
const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFileSync } = require('child_process');

const repoRoot = path.join(__dirname, '..');
const pkg = require(path.join(repoRoot, 'package.json'));

// npm resolves to npm.cmd on Windows. execFileSync can't launch it without a
// shell (ENOENT - unlike a real .exe such as git), and naming npm.cmd
// explicitly instead fails with EINVAL (a known Node/libuv quirk spawning
// batch files directly on Windows) - shell:true is the one combination that
// actually works. Node warns that shell:true concatenates args unescaped,
// which matters for untrusted input; every argument passed through
// npmExec below is either a fixed literal or a path this script generated
// itself via fs.mkdtempSync/npm's own --json output, never external input.
function npmExec(args, options) {
  return execFileSync('npm', args, { ...options, shell: true });
}

function ok(msg) {
  console.log(`release-check: ok - ${msg}`);
}

function fail(msg) {
  console.error(`release-check: FAIL - ${msg}`);
  process.exitCode = 1;
}

console.log('release-check: npm pack...');
const packOut = npmExec(['pack', '--json'], { cwd: repoRoot, encoding: 'utf8' });
const [packInfo] = JSON.parse(packOut);
const tarballPath = path.join(repoRoot, packInfo.filename);
if (!fs.existsSync(tarballPath)) {
  fail(`npm pack did not produce ${packInfo.filename}`);
  process.exit(1);
}
ok(`packed ${packInfo.filename} (${packInfo.entryCount} files, ${packInfo.size} bytes)`);

const home = fs.mkdtempSync(path.join(os.tmpdir(), 'win-nice-release-check-home-'));
const installPrefix = fs.mkdtempSync(path.join(os.tmpdir(), 'win-nice-release-check-npm-'));

try {
  npmExec(['install', tarballPath, '--no-save', '--prefix', installPrefix], {
    cwd: repoRoot,
    env: { ...process.env, WIN_NICE_HOME: home, WIN_NICE_NO_PATH: '1' },
    stdio: 'inherit',
  });

  const manifestPath = path.join(home, 'install-manifest.json');
  if (!fs.existsSync(manifestPath)) {
    fail('install-manifest.json missing after install - postinstall may not have run against the packed tarball');
  } else {
    const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
    if (manifest.version !== pkg.version) {
      fail(`installed manifest version ${manifest.version} does not match package.json ${pkg.version}`);
    } else {
      ok(`installed manifest version matches package.json (${pkg.version})`);
    }

    const expectedTools = [
      'idle', 'belownormal', 'abovenormal', 'high', 'realtime',
      'capc', 'capt', 'capm', 'admin', 'uiup', 'cy', 'cx',
    ];
    const missing = expectedTools.filter((t) => !manifest.files.includes(`${t}.ps1`));
    if (missing.length) {
      fail(`expected launcher(s) missing from the installed manifest: ${missing.join(', ')}`);
    } else {
      ok(`all ${expectedTools.length} expected launchers present in the installed manifest`);
    }
  }

  const capcPs1 = path.join(home, 'bin', 'capc.ps1');
  if (!fs.existsSync(capcPs1)) {
    fail('bin/capc.ps1 missing from the installed tarball');
  } else {
    let status = null;
    try {
      execFileSync('powershell', ['-NoProfile', '-File', capcPs1, '50', 'cmd.exe', '/c', 'exit', '7'], { stdio: 'ignore' });
      status = 0;
    } catch (err) {
      status = err.status;
    }
    if (status !== 7) {
      fail(`smoke test: "capc 50 cmd.exe /c exit 7" exited ${status}, expected 7`);
    } else {
      ok('smoke test: capc (installed from the tarball) propagates the wrapped exit code');
    }
  }
} finally {
  fs.rmSync(home, { recursive: true, force: true });
  fs.rmSync(installPrefix, { recursive: true, force: true });
  fs.rmSync(tarballPath, { force: true });
}

// CHANGELOG.md's topmost heading must be the version actually being
// released, and its compare-link base must be a git tag that really exists -
// this is exactly the class of drift round 11's review caught by hand
// (a CHANGELOG entry for a version that was never tagged or published).
const changelogPath = path.join(repoRoot, 'CHANGELOG.md');
const changelog = fs.readFileSync(changelogPath, 'utf8');
const headingMatch = changelog.match(/^## \[(\d+\.\d+\.\d+)\]/m);
if (!headingMatch) {
  fail('CHANGELOG.md: no "## [X.Y.Z]" version heading found');
} else if (headingMatch[1] !== pkg.version) {
  fail(`CHANGELOG.md's latest heading is [${headingMatch[1]}], but package.json version is ${pkg.version}`);
} else {
  ok(`CHANGELOG.md's latest heading matches package.json (${pkg.version})`);
}

const escapedVersion = pkg.version.replace(/\./g, '\\.');
const compareLinkPattern = new RegExp(`^\\[${escapedVersion}\\]: .*/compare/v([\\d.]+)\\.\\.\\.v${escapedVersion}\\s*$`, 'm');
const compareLinkMatch = changelog.match(compareLinkPattern);
if (!compareLinkMatch) {
  fail(`CHANGELOG.md: no "[${pkg.version}]: .../compare/vX...v${pkg.version}" reference link found`);
} else {
  const baseVersion = compareLinkMatch[1];
  let tags = [];
  try {
    tags = execFileSync('git', ['tag', '--list', 'v*'], { cwd: repoRoot, encoding: 'utf8' })
      .split('\n')
      .map((t) => t.trim())
      .filter(Boolean);
  } catch {
    // Not fatal on its own - the next check reports what it found (nothing).
  }
  if (!tags.includes(`v${baseVersion}`)) {
    fail(`CHANGELOG.md's [${pkg.version}] compare link uses base "v${baseVersion}", which is not an actual git tag (existing tags: ${tags.join(', ') || 'none'})`);
  } else {
    ok(`CHANGELOG.md's compare-base tag v${baseVersion} exists`);
  }
}

if (process.exitCode) {
  console.error('release-check: FAILED - see above');
} else {
  console.log('release-check: all checks passed');
}
