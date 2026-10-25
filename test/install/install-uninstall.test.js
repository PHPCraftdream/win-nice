'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const crypto = require('node:crypto');

const {
  install,
  TOOL_NAMES,
  EXE_NAME,
  SIDECAR_NAME,
  expectedInstalledFiles,
  shimText,
} = require('../../install/install');
const { uninstall } = require('../../install/uninstall');
const paths = require('../../install/paths');
const manifest = require('../../install/manifest');

const { FAKE_EXE, freshHome, withHome } = require('./home');

// The extensionless shim 0.2.x installed (verbatim apart from the tool name).
const OLD_SHIM = [
  '#!/bin/sh',
  '# SPDX-License-Identifier: MIT OR Apache-2.0',
  '# win-nice: managed-file',
  "MSYS2_ARG_CONV_EXCL='*'",
  'export MSYS2_ARG_CONV_EXCL',
  'script=$(cygpath -w "$(dirname "$0")/idle.ps1" 2>/dev/null) || script="$(dirname "$0")/idle.ps1"',
  'exec powershell -NoProfile -ExecutionPolicy Bypass -File "$script" "$@"',
  '',
].join('\n');

test('install creates exactly the 31 contract files and writes a manifest listing them', () => {
  withHome(freshHome(), () => {
    const result = install({ updatePath: false });
    assert.ok(result, 'install should not skip when WIN_NICE_HOME is set');
    const dir = paths.binDir();
    const expected = expectedInstalledFiles();
    assert.equal(expected.length, 31, 'contract: 31 installed file names');

    const files = fs.readdirSync(dir).sort();
    assert.deepEqual(files, expected.slice().sort());
    assert.deepEqual(result.files.slice().sort(), files);

    // The real exe and every tool .exe are byte-identical to the source exe.
    assert.ok(fs.readFileSync(path.join(dir, EXE_NAME)).equals(FAKE_EXE), 'wnice.exe bytes');
    for (const t of TOOL_NAMES) {
      assert.ok(fs.readFileSync(path.join(dir, `${t}.exe`)).equals(FAKE_EXE), `${t}.exe bytes`);
    }
    assert.equal(fs.readFileSync(path.join(dir, SIDECAR_NAME), 'utf8'), `${manifest.MARKER}\n`);
    for (const name of [...TOOL_NAMES, 'wnice']) {
      assert.equal(fs.readFileSync(path.join(dir, name), 'utf8'), shimText(name), `shim ${name} text`);
    }

    const data = manifest.read(paths.manifestPath());
    assert.ok(data);
    assert.equal(data.binDir, dir);
    assert.deepEqual(data.files, expected);
  });
});

test('install falls back to copyFileSync when fs.linkSync is unavailable', () => {
  withHome(freshHome(), () => {
    const originalLinkSync = fs.linkSync;
    fs.linkSync = function () {
      throw new Error('EPERM: hardlinks not supported on this filesystem');
    };
    try {
      install({ updatePath: false });
    } finally {
      fs.linkSync = originalLinkSync;
    }
    const dir = paths.binDir();
    assert.deepEqual(fs.readdirSync(dir).sort(), expectedInstalledFiles().slice().sort());
    for (const t of TOOL_NAMES) {
      assert.ok(
        fs.readFileSync(path.join(dir, `${t}.exe`)).equals(FAKE_EXE),
        `${t}.exe must be copied when hardlinking fails`
      );
    }
  });
});

test('install is idempotent: a second install over its own output leaves exactly the 31 correct files', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    install({ updatePath: false });
    const dir = paths.binDir();
    const expected = expectedInstalledFiles();
    assert.deepEqual(fs.readdirSync(dir).sort(), expected.slice().sort());
    assert.ok(fs.readFileSync(path.join(dir, EXE_NAME)).equals(FAKE_EXE));
    for (const t of TOOL_NAMES) {
      assert.ok(fs.readFileSync(path.join(dir, `${t}.exe`)).equals(FAKE_EXE), `${t}.exe bytes`);
    }
    assert.equal(fs.readFileSync(path.join(dir, SIDECAR_NAME), 'utf8'), `${manifest.MARKER}\n`);
    assert.deepEqual(manifest.read(paths.manifestPath()).files, expected);
  });
});

test('install (upgrade path) removes old-layout files listed in a synthetic previous manifest', () => {
  withHome(freshHome(), () => {
    const dir = paths.binDir();
    fs.mkdirSync(dir, { recursive: true });
    const oldFiles = ['idle.bat', 'idle.ps1', 'IdleLauncher.dll', 'IdleLauncher.dll.managed', 'load-launcher.ps1'];
    for (const name of oldFiles) fs.writeFileSync(path.join(dir, name), 'stale old layout');
    // The 0.2.x layout also had an extensionless 'idle' shim with DIFFERENT
    // text than today's (it exec'd idle.ps1); install() must replace it.
    fs.writeFileSync(path.join(dir, 'idle'), OLD_SHIM);
    manifest.write(paths.manifestPath(), { version: '0.2.2', binDir: dir, files: [...oldFiles, 'idle'] });

    install({ updatePath: false });

    for (const name of oldFiles) {
      assert.equal(fs.existsSync(path.join(dir, name)), false, `${name} (old layout) must be removed on upgrade`);
    }
    assert.deepEqual(fs.readdirSync(dir).sort(), expectedInstalledFiles().slice().sort());
    assert.equal(fs.readFileSync(path.join(dir, 'idle'), 'utf8'), shimText('idle'));
  });
});

test('install replaces an older version\'s marked shim even without a manifest', () => {
  withHome(freshHome(), () => {
    const dir = paths.binDir();
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(path.join(dir, 'capc'), OLD_SHIM);
    fs.writeFileSync(path.join(dir, 'idle'), '#!/bin/sh\necho a user script without the marker\n');

    assert.throws(() => install({ updatePath: false }), /refusing to overwrite a pre-existing unrelated file: .*idle/);
    fs.unlinkSync(path.join(dir, 'idle'));
    install({ updatePath: false });
    assert.equal(fs.readFileSync(path.join(dir, 'capc'), 'utf8'), shimText('capc'));
  });
});

// Regression: the tool exes were hardlinks of the npm package's own
// bin/wnice.exe, so a running tool locked that file and the next
// `npm install -g win-nice` failed with EBUSY inside node_modules.
test('tool exes are hardlinks of the installed wnice.exe, never of the package file', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    const dir = paths.binDir();
    const id = (p) => fs.statSync(p, { bigint: true }).ino;
    const installed = id(path.join(dir, EXE_NAME));
    const packageFile = id(path.join(process.env.WIN_NICE_SOURCE_BIN, EXE_NAME));
    assert.notEqual(installed, packageFile, 'wnice.exe must be a copy, not a link of the package file');
    for (const t of TOOL_NAMES) {
      const tool = id(path.join(dir, `${t}.exe`));
      assert.notEqual(tool, packageFile, `${t}.exe must not share the package file`);
      assert.equal(tool, installed, `${t}.exe must be a hardlink of the installed wnice.exe`);
    }
  });
});

test('install refuses to overwrite a pre-existing foreign capc.exe and leaves it byte-identical', () => {
  withHome(freshHome(), () => {
    const dir = paths.binDir();
    fs.mkdirSync(dir, { recursive: true });
    const foreignBytes = Buffer.from('a real binary the user put here, definitely not ours');
    const foreign = path.join(dir, 'capc.exe');
    fs.writeFileSync(foreign, foreignBytes);

    assert.throws(
      () => install({ updatePath: false }),
      /refusing to overwrite a pre-existing unrelated file/
    );
    assert.ok(fs.readFileSync(foreign).equals(foreignBytes), 'foreign file must stay byte-identical');
  });
});

test('uninstall without a manifest deletes owned files but never foreign ones', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    const dir = paths.binDir();
    fs.unlinkSync(paths.manifestPath());

    // Foreign files that happen to collide with managed names.
    const foreignExeBytes = Buffer.from('a foreign tool, not the win-nice binary');
    const foreignExe = path.join(dir, 'capc.exe');
    // capc.exe is a hardlink of the source wnice.exe (same NTFS inode), so
    // overwriting it in place would also rewrite the source exe - unlink first.
    fs.unlinkSync(foreignExe);
    fs.writeFileSync(foreignExe, foreignExeBytes);
    const foreignShim = path.join(dir, 'high');
    const foreignShimText = '#!/bin/sh\n# not win-nice\nexit 0\n';
    fs.writeFileSync(foreignShim, foreignShimText);

    // Drop the sidecar too, so ownership falls back to byte-equality - this
    // exercises the "bytes equal source exe" branch of isOwnedArtifact.
    fs.unlinkSync(path.join(dir, SIDECAR_NAME));

    uninstall({ updatePath: false });

    assert.ok(fs.readFileSync(foreignExe).equals(foreignExeBytes), 'foreign capc.exe must survive');
    assert.equal(fs.readFileSync(foreignShim, 'utf8'), foreignShimText, 'foreign shim must survive');
    assert.equal(fs.existsSync(path.join(dir, 'idle.exe')), false,
      'idle.exe (bytes equal the source exe) is ours and must be deleted');
  });
});

test('uninstall removes every file listed in the manifest and the manifest itself', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    const dir = paths.binDir();
    uninstall({ updatePath: false });
    assert.equal(fs.existsSync(dir), false);
    assert.equal(fs.existsSync(paths.manifestPath()), false);
  });
});

test('uninstall removes a manifest-tracked file even if its marker was stripped (user-modified)', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    const dir = paths.binDir();
    const target = path.join(dir, 'idle.exe');
    fs.writeFileSync(target, 'user replaced this binary');

    const results = uninstall({ updatePath: false });
    const idleResult = results.find((r) => r.file === target);
    assert.equal(idleResult.removed, true);
    assert.equal(fs.existsSync(target), false, 'manifest-tracked files are owned by the package');
  });
});

test('uninstall removes DLLs when fallback enumeration lists their marker sidecars first', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    fs.unlinkSync(paths.manifestPath());

    const dir = paths.binDir();
    assert.ok(fs.readdirSync(dir).length > 0);

    const originalReaddirSync = fs.readdirSync;
    fs.readdirSync = function (target, ...args) {
      const entries = originalReaddirSync.call(this, target, ...args);
      if (path.resolve(target) === path.resolve(dir) && entries.every((entry) => typeof entry === 'string')) {
        return entries.slice().sort().reverse();
      }
      return entries;
    };
    try {
      uninstall({ updatePath: false });
    } finally {
      fs.readdirSync = originalReaddirSync;
    }
    assert.equal(fs.existsSync(dir), false);
  });
});

test('uninstall fallback scan never removes an unmarked file dropped into binDir', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    fs.unlinkSync(paths.manifestPath());
    const dir = paths.binDir();
    const foreign = path.join(dir, 'not-ours.txt');
    fs.writeFileSync(foreign, 'unrelated user file');

    uninstall({ updatePath: false });
    assert.equal(fs.existsSync(foreign), true, 'unmarked foreign file must survive');
  });
});

test('uninstall rejects a manifest entry that traverses outside the install directory', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    const dir = paths.binDir();
    const manifestFile = paths.manifestPath();
    const data = manifest.read(manifestFile);

    const outsideTarget = path.join(dir, '..', 'outside.txt');
    fs.writeFileSync(outsideTarget, 'must survive');
    data.files.push('../outside.txt');
    manifest.write(manifestFile, data);

    const results = uninstall({ updatePath: false });
    assert.equal(fs.existsSync(outsideTarget), true, 'traversal target must survive uninstall');
    const rejected = results.find((r) => path.resolve(r.file) === path.resolve(outsideTarget));
    assert.equal(rejected.removed, false);
    fs.rmSync(outsideTarget, { force: true });
  });
});

test('uninstall rejects a manifest entry that is an absolute path outside the install directory', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    const manifestFile = paths.manifestPath();
    const data = manifest.read(manifestFile);

    const outsideTarget = fs.mkdtempSync(path.join(os.tmpdir(), 'win-nice-abs-'));
    const outsideFile = path.join(outsideTarget, 'evil.txt');
    fs.writeFileSync(outsideFile, 'must survive');
    data.files.push(outsideFile);
    manifest.write(manifestFile, data);

    const results = uninstall({ updatePath: false });
    assert.equal(fs.existsSync(outsideFile), true, 'absolute-path target must survive uninstall');
    const rejected = results.find((r) => r.reason === 'rejected (escapes install directory)');
    assert.ok(rejected, 'the absolute-path entry must be explicitly rejected, not just missed');
    fs.rmSync(outsideTarget, { recursive: true, force: true });
  });
});

test('reinstall (uninstall + install) leaves a clean, fully populated bin dir', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    uninstall({ updatePath: false });
    const result = install({ updatePath: false });
    const dir = paths.binDir();
    assert.deepEqual(fs.readdirSync(dir).sort(), result.files.slice().sort());
  });
});

test('install (upgrade path) removes a stale manifest-tracked file the current version no longer ships (e.g. a renamed tool)', () => {
  withHome(freshHome(), () => {
    const first = install({ updatePath: false });
    const dir = paths.binDir();

    // Simulate what a previous version would have left behind: a tool that
    // doesn't exist in the current bin/ (e.g. before a rename/removal).
    const staleNames = ['oldtool.bat', 'oldtool.ps1', 'oldtool'];
    for (const name of staleNames) fs.writeFileSync(path.join(dir, name), 'stale');
    const manifestFile = paths.manifestPath();
    const data = manifest.read(manifestFile);
    data.files = data.files.concat(staleNames);
    manifest.write(manifestFile, data);

    // `npm install -g win-nice@newer` only runs install() (postinstall) - it
    // never calls uninstall() first, unlike `win-nice reinstall`. This has to
    // be caught by cleanupStaleFiles() inside install() itself.
    const result = install({ updatePath: false });

    for (const name of staleNames) {
      assert.equal(fs.existsSync(path.join(dir, name)), false, `${name} (stale, no longer shipped) must be removed on upgrade`);
    }
    for (const name of first.files) {
      assert.equal(fs.existsSync(path.join(dir, name)), true, `${name} (still shipped) must survive`);
    }
    assert.deepEqual(
      manifest.read(manifestFile).files.slice().sort(),
      result.files.slice().sort(),
      'manifest must not still list the stale names'
    );
  });
});

test('install (upgrade path) fallback scan never removes a stale file that is merely MARKED but no longer owned (no manifest)', () => {
  withHome(freshHome(), () => {
    const first = install({ updatePath: false });
    const dir = paths.binDir();

    // Ownership no longer comes from the marker text alone: these names are
    // not part of the current tool set, so a marked leftover is foreign now.
    const staleNames = ['oldtool.bat', 'oldtool.ps1', 'oldtool'];
    for (const name of staleNames) fs.writeFileSync(path.join(dir, name), `# ${manifest.MARKER}\nstale\n`);
    fs.unlinkSync(paths.manifestPath());

    install({ updatePath: false });

    for (const name of staleNames) {
      assert.equal(fs.existsSync(path.join(dir, name)), true, `${name} (marked but not owned) must survive the fallback scan`);
    }
    for (const name of first.files) {
      assert.equal(fs.existsSync(path.join(dir, name)), true, `${name} (still shipped) must survive`);
    }
  });
});

test('install (upgrade path) fallback scan never removes a stale file that is merely MARKED but no longer owned (corrupt manifest)', () => {
  withHome(freshHome(), () => {
    const first = install({ updatePath: false });
    const dir = paths.binDir();

    const staleNames = ['oldtool.bat', 'oldtool.ps1', 'oldtool'];
    for (const name of staleNames) fs.writeFileSync(path.join(dir, name), `# ${manifest.MARKER}\nstale\n`);
    manifest.write(paths.manifestPath(), { version: '0.1.0', files: 'not-an-array' });

    install({ updatePath: false });

    for (const name of staleNames) {
      assert.equal(fs.existsSync(path.join(dir, name)), true, `${name} (marked but not owned) must survive the fallback scan`);
    }
    for (const name of first.files) {
      assert.equal(fs.existsSync(path.join(dir, name)), true, `${name} (still shipped) must survive`);
    }
  });
});

test('install (upgrade path) fallback scan never removes an unrelated UNMARKED file from binDir', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    const dir = paths.binDir();
    const foreign = path.join(dir, 'not-ours.txt');
    fs.writeFileSync(foreign, 'unrelated user file');
    fs.unlinkSync(paths.manifestPath());

    install({ updatePath: false });

    assert.equal(fs.existsSync(foreign), true, 'unmarked foreign file must survive the fallback scan');
  });
});

test('install() refreshes an existing marked skill copy on upgrade, not just via explicit `skill install`', () => {
  const { installSkill, targets } = require('../../install/skill');
  withHome(freshHome(), () => {
    installSkill();
    const [claudeTarget] = targets();
    const original = fs.readFileSync(claudeTarget, 'utf8');
    fs.writeFileSync(claudeTarget, original.replace('# win-nice', '# win-nice (STALE PRE-RENAME COPY)'));

    install({ updatePath: false });

    assert.equal(
      fs.readFileSync(claudeTarget, 'utf8'),
      original,
      'a previously-installed skill copy must be refreshed by a plain install(), not just `win-nice skill install`'
    );
  });
});

test('install is a no-op guard when run from the source checkout without WIN_NICE_HOME', () => {
  const prev = process.env.WIN_NICE_HOME;
  delete process.env.WIN_NICE_HOME;
  try {
    // updatePath: false - if this guard ever regresses, the test must not fall
    // through to a real PATH read/write against the developer's actual registry.
    const result = install({ updatePath: false });
    assert.equal(result, null);
  } finally {
    if (prev !== undefined) process.env.WIN_NICE_HOME = prev;
  }
});

// PID alone can be reused across runs/reboots, so a crashed run could leave a
// colliding scratch key behind; the GUID makes that impossible.
function scratchKey() {
  return `HKCU:\\Software\\WinNiceTest\\${process.pid}-${crypto.randomUUID()}`;
}

test('readRegistryString/writeRegistryString round-trip non-ASCII values exactly, on a scratch key', () => {
  const keyPath = scratchKey();
  const valueName = 'ScratchPath';
  const value = 'C:\\Users\\Марат\\bin;C:\\Users\\José\\bin';
  try {
    paths.writeRegistryString(keyPath, valueName, value);
    const readBack = paths.readRegistryString(keyPath, valueName);
    assert.equal(readBack, value);
  } finally {
    execFileSync('powershell', [
      '-NoProfile',
      '-Command',
      'Remove-Item -LiteralPath $env:WIN_NICE_REG_KEY -Recurse -Force -ErrorAction SilentlyContinue',
    ], { env: { ...process.env, WIN_NICE_REG_KEY: keyPath } });
  }
});

test('writeRegistryString preserves REG_EXPAND_SZ across a round trip instead of flattening it', () => {
  const keyPath = scratchKey();
  const valueName = 'ScratchExpand';
  try {
    execFileSync('powershell', [
      '-NoProfile',
      '-Command',
      [
        'if (-not (Test-Path -LiteralPath $env:WIN_NICE_REG_KEY)) { New-Item -Path $env:WIN_NICE_REG_KEY -Force | Out-Null }',
        'Set-ItemProperty -LiteralPath $env:WIN_NICE_REG_KEY -Name $env:WIN_NICE_REG_VALUE -Value $env:WIN_NICE_REG_NEW_VALUE -Type ExpandString',
      ].join('\n'),
    ], {
      env: { ...process.env, WIN_NICE_REG_KEY: keyPath, WIN_NICE_REG_VALUE: valueName, WIN_NICE_REG_NEW_VALUE: '%USERPROFILE%\\bin' },
    });

    // Round-trip through writeRegistryString, as install()/uninstall() do.
    paths.writeRegistryString(keyPath, valueName, '%USERPROFILE%\\bin2;C:\\extra');

    const kind = execFileSync('powershell', [
      '-NoProfile',
      '-Command',
      '(Get-Item -LiteralPath $env:WIN_NICE_REG_KEY).GetValueKind($env:WIN_NICE_REG_VALUE)',
    ], { encoding: 'utf8', env: { ...process.env, WIN_NICE_REG_KEY: keyPath, WIN_NICE_REG_VALUE: valueName } });
    assert.equal(kind.trim(), 'ExpandString');

    const raw = paths.readRegistryString(keyPath, valueName);
    assert.equal(raw, '%USERPROFILE%\\bin2;C:\\extra', 'value must stay unexpanded');
  } finally {
    execFileSync('powershell', [
      '-NoProfile',
      '-Command',
      'Remove-Item -LiteralPath $env:WIN_NICE_REG_KEY -Recurse -Force -ErrorAction SilentlyContinue',
    ], { env: { ...process.env, WIN_NICE_REG_KEY: keyPath } });
  }
});

test('addToPathString/removeFromPathString treat a raw %LOCALAPPDATA% entry as its expanded equivalent', () => {
  const expanded = path.join(process.env.LOCALAPPDATA, 'win-nice', 'bin');
  const raw = '%LOCALAPPDATA%\\win-nice\\bin';
  assert.equal(paths.addToPathString(raw, expanded), raw, 'add must dedup against the raw equivalent, not append a duplicate');
  assert.equal(paths.removeFromPathString(raw, expanded), '', 'remove must strip the raw equivalent entry');
});

test('addToPathString/removeFromPathString treat a raw %USERPROFILE% entry as its expanded equivalent', () => {
  const expanded = path.join(process.env.USERPROFILE, 'somewhere', 'bin');
  const raw = '%USERPROFILE%\\somewhere\\bin';
  assert.equal(paths.addToPathString(raw, expanded), raw);
  assert.equal(paths.removeFromPathString(raw, expanded), '');
});

test('%VAR% name matching is case-insensitive (%localappdata% matches %LOCALAPPDATA%)', () => {
  const expanded = path.join(process.env.LOCALAPPDATA, 'win-nice', 'bin');
  const raw = '%localappdata%\\win-nice\\bin';
  assert.equal(paths.addToPathString(raw, expanded), raw);
  assert.equal(paths.removeFromPathString(raw, expanded), '');
});

test('unknown %VAR% stays literal text and never matches an unrelated directory', () => {
  const expanded = path.join(process.env.LOCALAPPDATA, 'win-nice', 'bin');
  const raw = '%NOT_A_REAL_VAR%\\win-nice\\bin';
  const added = paths.addToPathString(raw, expanded);
  assert.equal(added, `${raw};${expanded}`, 'unexpandable entry must not dedup against the expanded dir');
  assert.equal(paths.removeFromPathString(added, expanded), raw, 'removal must drop only the expanded entry and keep the literal one');
  assert.equal(paths.removeFromPathString(raw, path.join('C:', 'nope', 'bin')), raw, 'literal entry survives removal of a different directory');
});

// ---- rename-aside / .old-N leftover coverage ----

const { isOldLeftoverName } = require('../../install/install');

test('isOldLeftoverName matches the exact `<tool>.exe.old-<digits>` pattern only', () => {
  assert.equal(isOldLeftoverName('wnice.exe.old-12'), true);
  assert.equal(isOldLeftoverName('idle.exe.old-1'), true);
  assert.equal(isOldLeftoverName('capn.exe.old-42'), true);
  assert.equal(isOldLeftoverName('idle.old-1'), false);
  assert.equal(isOldLeftoverName('idle.exe.old'), false);
  assert.equal(isOldLeftoverName('xidle.exe.old-1'), false);
  assert.equal(isOldLeftoverName('evil.exe.old-3'), false);
  assert.equal(isOldLeftoverName('capm.exe.old-x'), false);
  assert.equal(isOldLeftoverName('wnice.exe'), false);
});

test('upgrade rolls back and reports close-running-tools when a Phase C step fails', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    const dir = paths.binDir();

    let copyCalls = 0;
    const _fs = new Proxy(fs, {
      get(target, prop) {
        if (prop === 'copyFileSync') {
          return (...args) => {
            copyCalls++;
            if (copyCalls === 1) {
              throw Object.assign(new Error('EPERM: locked'), { code: 'EPERM' });
            }
            return target.copyFileSync(...args);
          };
        }
        return target[prop];
      },
    });

    assert.throws(
      () => install({ updatePath: false, _fs }),
      /close running win-nice tools/
    );

    // Rollback must have restored the pre-upgrade dir exactly, with no .old-* left.
    assert.deepEqual(fs.readdirSync(dir).sort(), expectedInstalledFiles().slice().sort());
    assert.ok(fs.readFileSync(path.join(dir, EXE_NAME)).equals(FAKE_EXE), 'pre-upgrade wnice.exe must be restored');
  });
});

test('phase R renames existing exes aside and post-success cleanup removes all .old-N leftovers', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    const dir = paths.binDir();
    for (const name of ['capc.exe.old-1', 'capc.exe.old-2', 'idle.exe.old-7']) {
      fs.writeFileSync(path.join(dir, name), 'stale old bytes');
    }

    install({ updatePath: false });

    assert.deepEqual(fs.readdirSync(dir).sort(), expectedInstalledFiles().slice().sort());
    assert.equal(fs.existsSync(path.join(dir, 'capc.exe.old-1')), false);
    assert.equal(fs.existsSync(path.join(dir, 'capc.exe.old-2')), false);
    assert.equal(fs.existsSync(path.join(dir, 'idle.exe.old-7')), false);
    assert.ok(fs.readFileSync(path.join(dir, 'capc.exe')).equals(FAKE_EXE));
    assert.ok(fs.readFileSync(path.join(dir, 'idle.exe')).equals(FAKE_EXE));
  });
});

test('leftover cleanup matches the exact pattern only', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    const dir = paths.binDir();
    for (const name of ['evil.exe.old-3', 'capm.exe.old-x', 'xidle.exe.old-1', 'capn.exe.old-42']) {
      fs.writeFileSync(path.join(dir, name), 'seed');
    }

    const result = install({ updatePath: false });

    assert.equal(fs.existsSync(path.join(dir, 'capn.exe.old-42')), false, 'exact-pattern leftover must be removed');
    assert.equal(fs.existsSync(path.join(dir, 'evil.exe.old-3')), true, 'evil.exe.old-3 must survive');
    assert.equal(fs.existsSync(path.join(dir, 'capm.exe.old-x')), true, 'capm.exe.old-x must survive');
    assert.equal(fs.existsSync(path.join(dir, 'xidle.exe.old-1')), true, 'xidle.exe.old-1 must survive');
    for (const name of result.files) {
      assert.ok(!name.includes('.old-'), `manifest must never list a .old- file: ${name}`);
    }
  });
});

test('uninstall survives a locked exe by renaming it aside', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    const dir = paths.binDir();

    const originalUnlinkSync = fs.unlinkSync;
    // The image lock travels with the file identity, so the renamed-aside
    // wnice.exe.old-1 is locked too - not just the .exe names.
    fs.unlinkSync = function (target, ...args) {
      if (/\.exe(\.old-\d+)?$/.test(String(target))) {
        throw Object.assign(new Error('EPERM: in use'), { code: 'EPERM' });
      }
      return originalUnlinkSync.call(this, target, ...args);
    };
    let results;
    try {
      results = uninstall({ updatePath: false });
    } finally {
      fs.unlinkSync = originalUnlinkSync;
    }

    assert.ok(results.some((r) => /in use/.test(r.reason)), 'a locked exe must be reported as in use');
    assert.equal(fs.existsSync(dir), true, 'dir stays behind while .old-N files remain');
    for (const entry of fs.readdirSync(dir)) {
      assert.match(entry, /\.old-\d+$/, `only .old-N files may remain, found: ${entry}`);
    }

    // Second pass, no lock: everything (including the .old-N files) is removable.
    uninstall({ updatePath: false });
    assert.equal(fs.existsSync(dir), false, 'dir must be gone once the leftovers are deletable');
  });
});
