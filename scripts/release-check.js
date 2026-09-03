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

      // Full contract: each of the 12 tools ships exactly 3 entry points
      // (extensionless Git Bash shim, .bat, .ps1) - 36 files total - and the 6
      // pre-rename legacy names (cap/pint, renamed to capc/capt) must never
      // reappear in a real install.
      const expectedTools = [
        'idle', 'belownormal', 'abovenormal', 'high', 'realtime',
        'capc', 'capt', 'capm', 'admin', 'uiup', 'cy', 'cx',
      ];
      const expectedFiles = expectedTools.flatMap((t) => [t, `${t}.bat`, `${t}.ps1`]);
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
        fail(`installed manifest does not match the exact 36-file launcher contract (${parts.join('; ')})`);
      } else {
        ok(`all ${expectedFiles.length} expected launcher files present (12 tools x 3 variants), no legacy names`);
      }
    }

    // The manifest/exact-file-set check above only proves the files exist,
    // not that each one actually runs - exercise one .ps1 per Job-Object
    // launcher family (capc/capt/capm each wrap process creation
    // differently), not just capc, so a capt/capm-only regression can't slip
    // through a gate that only ever smoke-tested capc.
    const smokeCases = [
      { tool: 'capc', arg: '50' },
      { tool: 'capt', arg: '1' },
      { tool: 'capm', arg: '50' },
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
