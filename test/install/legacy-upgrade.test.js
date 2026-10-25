'use strict';
// Regression tests for upgrading/uninstalling over a real 0.2.x install (71
// PowerShell/.bat/DLL files). 0.3.0's first `npm install -g` over such an
// install refused with "refusing to overwrite a pre-existing unrelated file:
// ...\bin\idle", because the old extensionless shims had other text than the
// new ones.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const { install, TOOL_NAMES, expectedInstalledFiles, shimText } = require('../../install/install');
const { uninstall } = require('../../install/uninstall');
const paths = require('../../install/paths');
const manifest = require('../../install/manifest');
const { freshHome, withHome } = require('./home');

const DLL_CLASSES = [
  'AboveNormalLauncher', 'AdminLauncher', 'BelowNormalLauncher', 'CapcLauncher', 'CapmLauncher',
  'CapnLauncher', 'CapsLauncher', 'CaptLauncher', 'CxLauncher', 'CyLauncher', 'HighLauncher',
  'IdleLauncher', 'RealtimeLauncher', 'EnvironmentNotifier',
];

// Same shapes 0.2.2 shipped: every text file carries the marker comment, a
// DLL is unmarked binary data whose .managed sidecar carries it.
function oldShim(tool) {
  return [
    '#!/bin/sh',
    '# SPDX-License-Identifier: MIT OR Apache-2.0',
    '# win-nice: managed-file',
    "MSYS2_ARG_CONV_EXCL='*'",
    'export MSYS2_ARG_CONV_EXCL',
    `script=$(cygpath -w "$(dirname "$0")/${tool}.ps1" 2>/dev/null) || script="$(dirname "$0")/${tool}.ps1"`,
    'exec powershell -NoProfile -ExecutionPolicy Bypass -File "$script" "$@"',
    '',
  ].join('\n');
}

function oldLayout() {
  const files = {};
  for (const t of TOOL_NAMES) {
    files[t] = oldShim(t);
    files[`${t}.bat`] = `@echo off\r\n:: SPDX-License-Identifier: MIT OR Apache-2.0\r\n:: win-nice: managed-file\r\n`;
    files[`${t}.ps1`] = `# SPDX-License-Identifier: MIT OR Apache-2.0\r\n# win-nice: managed-file\r\n`;
  }
  files['load-launcher.ps1'] = '# win-nice: managed-file\r\n$script:WinNiceLauncherDirectory = $PSScriptRoot\r\n';
  for (const c of DLL_CLASSES) {
    files[`${c}.dll`] = Buffer.from([0x4d, 0x5a, 0x90, 0x00, ...Buffer.from(c)]);
    files[`${c}.dll.managed`] = '# win-nice: managed-file\n# source-sha256: 00\n';
  }
  return files;
}

function seedOldInstall(dir, { withManifest }) {
  fs.mkdirSync(dir, { recursive: true });
  const files = oldLayout();
  for (const [name, content] of Object.entries(files)) fs.writeFileSync(path.join(dir, name), content);
  if (withManifest) {
    manifest.write(paths.manifestPath(), {
      version: '0.2.2',
      installedAt: '2026-09-27T00:00:00.000Z',
      binDir: dir,
      files: Object.keys(files),
    });
  }
  return Object.keys(files);
}

test('the fixture matches the 0.2.x install: 71 files', () => {
  assert.equal(Object.keys(oldLayout()).length, 71);
});

for (const withManifest of [true, false]) {
  const how = withManifest ? 'with its manifest' : 'without a manifest';

  test(`npm upgrade over a full 0.2.x install ${how} leaves exactly the new layout`, () => {
    withHome(freshHome(), () => {
      const dir = paths.binDir();
      seedOldInstall(dir, { withManifest });

      install({ updatePath: false });

      assert.deepEqual(fs.readdirSync(dir).sort(), expectedInstalledFiles().slice().sort());
      for (const t of TOOL_NAMES) {
        assert.equal(fs.readFileSync(path.join(dir, t), 'utf8'), shimText(t), `${t} shim replaced`);
      }
      assert.equal(manifest.read(paths.manifestPath()).files.length, expectedInstalledFiles().length);
    });
  });

  test(`upgrade over a 0.2.x install ${how} never touches unmarked files of the user`, () => {
    withHome(freshHome(), () => {
      const dir = paths.binDir();
      seedOldInstall(dir, { withManifest });
      // Same names as old managed files, but the marker is gone: the user's own.
      const userFiles = {
        'notes.txt': 'mine',
        'my-tool.ps1': 'Write-Host mine',
      };
      if (!withManifest) {
        userFiles['capc.ps1'] = 'Write-Host my own capc';
        userFiles['IdleLauncher.dll.managed'] = 'my own file';
      }
      for (const [name, content] of Object.entries(userFiles)) fs.writeFileSync(path.join(dir, name), content);

      install({ updatePath: false });

      for (const [name, content] of Object.entries(userFiles)) {
        assert.equal(fs.readFileSync(path.join(dir, name), 'utf8'), content, `${name} must survive`);
      }
    });
  });
}

test('uninstall (current code) removes a 0.2.x install that has no manifest', () => {
  withHome(freshHome(), () => {
    const dir = paths.binDir();
    seedOldInstall(dir, { withManifest: false });
    fs.writeFileSync(path.join(dir, 'notes.txt'), 'mine');

    uninstall({ updatePath: false });

    assert.deepEqual(fs.readdirSync(dir), ['notes.txt']);
  });
});

test('a DLL is removed before the sidecar that proves it is ours', () => {
  withHome(freshHome(), () => {
    const dir = paths.binDir();
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(path.join(dir, 'IdleLauncher.dll'), Buffer.from([0x4d, 0x5a]));
    fs.writeFileSync(path.join(dir, 'IdleLauncher.dll.managed'), '# win-nice: managed-file\n');

    uninstall({ updatePath: false });

    assert.equal(fs.existsSync(dir) ? fs.readdirSync(dir).length : 0, 0);
  });
});
