'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const CLI = path.join(__dirname, '..', 'install', 'cli.js');

function run(args, home) {
  return spawnSync(process.execPath, [CLI, ...args], {
    encoding: 'utf8',
    env: { ...process.env, WIN_NICE_HOME: home, WIN_NICE_NO_PATH: '1' },
  });
}

function withHome(fn) {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'win-nice-cli-'));
  try {
    fn(home);
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
}

test('cli status reports "not installed" against a fresh home', () => {
  withHome((home) => {
    const res = run(['status'], home);
    assert.equal(res.status, 0);
    assert.match(res.stdout, /not installed/);
  });
});

test('cli install then status reports the installed version and files', () => {
  withHome((home) => {
    const installResult = run(['install'], home);
    assert.equal(installResult.status, 0, installResult.stderr);

    const res = run(['status'], home);
    assert.match(res.stdout, /win-nice .* installed/);
    assert.match(res.stdout, /idle\.bat/);
  });
});

test('cli uninstall removes a prior install', () => {
  withHome((home) => {
    run(['install'], home);
    const uninstallResult = run(['uninstall'], home);
    assert.equal(uninstallResult.status, 0, uninstallResult.stderr);

    const res = run(['status'], home);
    assert.match(res.stdout, /not installed/);
  });
});

test('cli rejects an unknown command with a non-zero exit code', () => {
  withHome((home) => {
    const res = run(['bogus'], home);
    assert.notEqual(res.status, 0);
    assert.match(res.stderr, /unknown command/);
  });
});
