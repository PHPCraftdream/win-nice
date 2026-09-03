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
// batch files directly on Windows). shell:true "worked" but concatenates the
// args array unescaped (Node's own DEP0190 warning) - repoRoot and
// os.tmpdir() both come from the ambient environment and can contain spaces
// (e.g. "C:\Users\Jane Doe\...") or shell metacharacters, which silently
// truncated/broke the command. Bypass the shell entirely instead: run node
// directly against npm's own JS entrypoint (process.env.npm_execpath), which
// `npm run` always sets.
function npmExec(args, options) {
  const npmExecPath = process.env.npm_execpath;
  if (!npmExecPath) {
    console.error(
      'release-check: npm_execpath is not set - run this via "npm run release-check", not "node scripts/release-check.js" directly.'
    );
    process.exit(1);
  }
  return execFileSync(process.execPath, [npmExecPath, ...args], options);
}

function ok(msg) {
  console.log(`release-check: ok - ${msg}`);
}

function fail(msg) {
  console.error(`release-check: FAIL - ${msg}`);
  process.exitCode = 1;
}

// Full contract: each of the 14 tools ships exactly 3 entry points
// (extensionless Git Bash shim, .bat, .ps1) - 42 files total - and the 6
// pre-rename legacy names (cap/pint, renamed to capc/capt) must never
// reappear in a real install.
const expectedTools = [
  'idle', 'belownormal', 'abovenormal', 'high', 'realtime',
  'capc', 'capt', 'capm', 'caps', 'capn', 'admin', 'uiup', 'cy', 'cx',
];
const expectedFiles = expectedTools.flatMap((t) => [t, `${t}.bat`, `${t}.ps1`]);

// Same 42 paths the bin/ check below compares, hoisted here so the
// whole-tarball allowlist can build on them (and so
// test/release-check-allowlist.test.js can import the full contract without
// running the gate).
const expectedBinPaths = expectedFiles.map((f) => `bin/${f}`);

// package.json's `files` whitelist ships install/ and skills/ whole too, so
// a stray file anywhere - not just bin/ - gets packed into the real
// published tarball. Deliberately a hardcoded literal list, not a scan of
// install/ and skills/: the point is to catch a file that exists on disk but
// must not ship, and a scan would just bless whatever happens to be there.
// 54 paths = 42 launchers + 6 install/*.js + skills/win-nice/SKILL.md +
// package.json (always packed by npm even though it is not listed in
// `files`) + the 4 whitelisted root docs/licenses.
const expectedNonBinPaths = [
  'install/cli.js',
  'install/install.js',
  'install/manifest.js',
  'install/paths.js',
  'install/skill.js',
  'install/uninstall.js',
  'skills/win-nice/SKILL.md',
  'package.json',
  'README.md',
  'CHANGELOG.md',
  'LICENSE-MIT',
  'LICENSE-APACHE',
];
const expectedTarballPaths = [...expectedBinPaths, ...expectedNonBinPaths];

function diffTarballPaths(packedPaths) {
  return {
    missing: expectedTarballPaths.filter((p) => !packedPaths.includes(p)),
    extra: packedPaths.filter((p) => !expectedTarballPaths.includes(p)),
  };
}

// Required as a module (test/release-check-allowlist.test.js): expose the
// pure allowlist data/comparator without running the gate. A top-level
// return is valid CommonJS; executed directly (npm run release-check),
// execution falls through to the gate below.
if (require.main !== module) {
  module.exports = { expectedTarballPaths, diffTarballPaths };
  return;
}

// Resources are declared before the try so a failure during npm pack itself
// (bad JSON, npm error, mkdtempSync failure) still lets the finally clean up
// whatever was actually created, instead of leaking temp dirs/tarball.
let tarballPath = null;
let home = null;
let installPrefix = null;
let skillHome = null;

try {
  console.log('release-check: npm pack...');
  const packOut = npmExec(['pack', '--json'], { cwd: repoRoot, encoding: 'utf8' });
  const [packInfo] = JSON.parse(packOut);
  tarballPath = path.join(repoRoot, packInfo.filename);
  if (!fs.existsSync(tarballPath)) {
    fail(`npm pack did not produce ${packInfo.filename}`);
  } else {
    ok(`packed ${packInfo.filename} (${packInfo.entryCount} files, ${packInfo.size} bytes)`);

    // package.json's `files` whitelist ships the whole bin/ directory, so any
    // stray file dropped in there gets packed - but install/'s
    // listSourceFiles() only installs .bat/.ps1/extensionless names, so the
    // installed-manifest check below would still pass green and never notice.
    // Compare the tarball's actual file list against the same 42-path
    // allowlist: bin/ must be exactly the launcher contract, nothing more.
    const packedAllPaths = (Array.isArray(packInfo.files) ? packInfo.files : [])
      .map((f) => f.path);
    const packedBinPaths = packedAllPaths.filter((p) => p.startsWith('bin/'));
    const packedMissing = expectedBinPaths.filter((p) => !packedBinPaths.includes(p));
    const packedExtra = packedBinPaths.filter((p) => !expectedBinPaths.includes(p));
    if (packedMissing.length || packedExtra.length) {
      const parts = [];
      if (packedMissing.length) parts.push(`missing from tarball: ${packedMissing.join(', ')}`);
      if (packedExtra.length) parts.push(`unexpected in tarball: ${packedExtra.join(', ')}`);
      fail(`packed tarball's bin/ does not match the exact ${expectedFiles.length}-file launcher contract (${parts.join('; ')})`);
    } else {
      ok(`packed tarball's bin/ contains exactly the ${expectedFiles.length} expected launcher files`);
    }

    // The bin/ check above stays green when the mismatch is anywhere else in
    // the tarball (a stray install/ or skills/ file, a missing root doc), so
    // compare the FULL packed list against the exact allowlist too. Same
    // comparison, same message style - the bin/-specific result above says
    // whether the launchers specifically drifted.
    const tarballDiff = diffTarballPaths(packedAllPaths);
    if (tarballDiff.missing.length || tarballDiff.extra.length) {
      const parts = [];
      if (tarballDiff.missing.length) parts.push(`missing from tarball: ${tarballDiff.missing.join(', ')}`);
      if (tarballDiff.extra.length) parts.push(`unexpected in tarball: ${tarballDiff.extra.join(', ')}`);
      fail(`packed tarball does not match the exact ${expectedTarballPaths.length}-file allowlist (${parts.join('; ')})`);
    } else {
      ok(`packed tarball contains exactly the ${expectedTarballPaths.length} expected files`);
    }

    home = fs.mkdtempSync(path.join(os.tmpdir(), 'win-nice-release-check-home-'));
    installPrefix = fs.mkdtempSync(path.join(os.tmpdir(), 'win-nice-release-check-npm-'));
    // The tarball's postinstall calls install(), which also calls
    // updateInstalledSkill() - that resolves its targets via the SEPARATE
    // WIN_NICE_SKILL_HOME env var, not WIN_NICE_HOME. Without this, the
    // tarball install below is free to rewrite the real ~/.claude/skills and
    // ~/.agents/skills (confirmed: it did, before this temp dir was added).
    skillHome = fs.mkdtempSync(path.join(os.tmpdir(), 'win-nice-release-check-skillhome-'));

    npmExec(['install', tarballPath, '--no-save', '--prefix', installPrefix], {
      cwd: repoRoot,
      env: { ...process.env, WIN_NICE_HOME: home, WIN_NICE_SKILL_HOME: skillHome, WIN_NICE_NO_PATH: '1' },
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

      const legacyNames = ['cap', 'cap.bat', 'cap.ps1', 'pint', 'pint.bat', 'pint.ps1'];
      const manifestFiles = Array.isArray(manifest.files) ? manifest.files : [];
      const missing = expectedFiles.filter((f) => !manifestFiles.includes(f));
      const extra = manifestFiles.filter((f) => !expectedFiles.includes(f) && !legacyNames.includes(f));
      const forbidden = legacyNames.filter((f) => manifestFiles.includes(f));
      if (missing.length || extra.length || forbidden.length) {
        const parts = [];
        if (missing.length) parts.push(`missing: ${missing.join(', ')}`);
        if (extra.length) parts.push(`extra: ${extra.join(', ')}`);
        if (forbidden.length) parts.push(`forbidden legacy name(s) present: ${forbidden.join(', ')}`);
        fail(`installed manifest does not match the exact 42-file launcher contract (${parts.join('; ')})`);
      } else {
        ok(`all ${expectedFiles.length} expected launcher files present (14 tools x 3 variants), no legacy names`);
      }
    }

    // The manifest/exact-file-set check above only proves the files exist,
    // not that each one actually runs - exercise one .ps1 per Job-Object
    // launcher family (capc/capt/capm each wrap process creation
    // differently; caps wraps the same process-creation machinery with a
    // bounded wait instead of an infinite one; capn sets an
    // active-process-count limit instead), not just capc, so a
    // capt/capm/caps/capn-only regression can't slip
    // through a gate that only ever smoke-tested capc.
    const smokeCases = [
      { tool: 'capc', arg: '50' },
      { tool: 'capt', arg: '1' },
      { tool: 'capm', arg: '50' },
      { tool: 'caps', arg: '30' },
      { tool: 'capn', arg: '10' },
    ];
    for (const { tool, arg } of smokeCases) {
      const ps1 = path.join(home, 'bin', `${tool}.ps1`);
      if (!fs.existsSync(ps1)) {
        fail(`bin/${tool}.ps1 missing from the installed tarball`);
        continue;
      }
      let status = null;
      let stderr = '';
      try {
        // Match the -ExecutionPolicy Bypass every .bat/shim wrapper already
        // passes: without it, a default Restricted client policy makes the
        // .ps1 itself refuse to run, and stdio:'ignore' used to hide that
        // entirely behind a bare "exited 1, expected 7".
        execFileSync(
          'powershell',
          ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', ps1, arg, 'cmd.exe', '/c', 'exit', '7'],
          { stdio: ['ignore', 'ignore', 'pipe'], encoding: 'utf8' }
        );
        status = 0;
      } catch (err) {
        status = err.status;
        stderr = err.stderr ? err.stderr.trim() : '';
      }
      if (status !== 7) {
        fail(`smoke test: "${tool} ${arg} cmd.exe /c exit 7" exited ${status}, expected 7${stderr ? ` (stderr: ${stderr})` : ''}`);
      } else {
        ok(`smoke test: ${tool} (installed from the tarball) propagates the wrapped exit code`);
      }
    }
    // Every tool ships three launcher variants but the loop above only ever
    // exercises the .ps1 one; run one .bat and one extensionless Git Bash shim
    // from the installed tarball too, so a variant-only regression can't pass
    // the gate either. idle takes the wrapped command directly (no numeric
    // size/percent argument like the capc/capt/capm launchers).
    const bat = path.join(home, 'bin', 'idle.bat');
    if (!fs.existsSync(bat)) {
      fail('bin/idle.bat missing from the installed tarball');
    } else {
      let status = null;
      let stderr = '';
      try {
        execFileSync('cmd.exe', ['/d', '/c', bat, 'cmd.exe', '/c', 'exit', '7'], {
          stdio: ['ignore', 'ignore', 'pipe'],
          encoding: 'utf8',
        });
        status = 0;
      } catch (err) {
        status = err.status;
        stderr = err.stderr ? err.stderr.trim() : '';
      }
      if (status !== 7) {
        fail(`smoke test: "idle.bat cmd.exe /c exit 7" exited ${status}, expected 7${stderr ? ` (stderr: ${stderr})` : ''}`);
      } else {
        ok('smoke test: idle.bat (installed from the tarball) propagates the wrapped exit code');
      }
    }

    // Same guard as test/gitbash-shims.test.js: `bash` on PATH isn't
    // necessarily Git Bash/MSYS (WSL ships one too and can't run these
    // Windows-path shims) - skip with a note instead of failing confusingly.
    let hasGitBash = false;
    try {
      const uname = execFileSync('bash', ['-c', 'uname -s'], {
        stdio: ['ignore', 'pipe', 'ignore'],
        encoding: 'utf8',
      });
      hasGitBash = /^(MINGW|MSYS)/.test(uname.trim());
    } catch {
      hasGitBash = false;
    }
    const shim = path.join(home, 'bin', 'idle').replace(/\\/g, '/');
    if (!hasGitBash) {
      console.log('release-check: note - no Git Bash/MSYS on PATH; skipping the extensionless-shim smoke test');
    } else if (!fs.existsSync(shim)) {
      fail('bin/idle (extensionless shim) missing from the installed tarball');
    } else {
      let status = null;
      let stderr = '';
      try {
        execFileSync('bash', [shim, 'cmd.exe', '/c', 'exit', '7'], {
          stdio: ['ignore', 'ignore', 'pipe'],
          encoding: 'utf8',
        });
        status = 0;
      } catch (err) {
        status = err.status;
        stderr = err.stderr ? err.stderr.trim() : '';
      }
      if (status !== 7) {
        fail(`smoke test: "idle cmd.exe /c exit 7" (extensionless shim via Git Bash) exited ${status}, expected 7${stderr ? ` (stderr: ${stderr})` : ''}`);
      } else {
        ok('smoke test: idle extensionless shim (installed from the tarball, via Git Bash) propagates the wrapped exit code');
      }
    }
  }
} finally {
  if (tarballPath) fs.rmSync(tarballPath, { force: true });
  if (home) fs.rmSync(home, { recursive: true, force: true });
  if (installPrefix) fs.rmSync(installPrefix, { recursive: true, force: true });
  if (skillHome) fs.rmSync(skillHome, { recursive: true, force: true });
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
