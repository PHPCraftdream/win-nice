'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const { checkAssemblies } = require('../scripts/check-assemblies');
const { HELPER_ASSEMBLIES } = require('../install/install');

const repoRoot = path.join(__dirname, '..');
const realBinDir = path.join(repoRoot, 'bin');
const realScriptsDir = path.join(repoRoot, 'scripts');

test('check-assemblies: real repo has all 14 helper assemblies up to date', () => {
  const { ok, stale } = checkAssemblies();
  assert.deepEqual(stale, [], `stale/missing assemblies: ${JSON.stringify(stale)}`);
  assert.equal(ok, true);
});

// Copies exactly the files checkAssemblies needs into a scratch dir, so the
// tests below can mutate one file at a time without touching the real repo.
function makeFixture() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'win-nice-check-assemblies-'));
  const binDir = path.join(dir, 'bin');
  const scriptsDir = path.join(dir, 'scripts');
  fs.mkdirSync(binDir);
  fs.mkdirSync(scriptsDir);
  for (const { class: className, script } of HELPER_ASSEMBLIES) {
    if (script) fs.copyFileSync(path.join(realBinDir, script), path.join(binDir, script));
    fs.copyFileSync(path.join(realBinDir, `${className}.dll`), path.join(binDir, `${className}.dll`));
    fs.copyFileSync(path.join(realBinDir, `${className}.dll.managed`), path.join(binDir, `${className}.dll.managed`));
  }
  fs.copyFileSync(
    path.join(realScriptsDir, 'EnvironmentNotifier.cs'),
    path.join(scriptsDir, 'EnvironmentNotifier.cs')
  );
  return { dir, binDir, scriptsDir };
}

function withFixture(fn) {
  const { dir, binDir, scriptsDir } = makeFixture();
  try {
    fn({ binDir, scriptsDir });
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

test('check-assemblies: a clean fixture copy of the real repo is also clean', () => {
  withFixture(({ binDir, scriptsDir }) => {
    const { ok } = checkAssemblies({ binDirectory: binDir, scriptsDirectory: scriptsDir });
    assert.equal(ok, true);
  });
});

test('check-assemblies: detects a stale sidecar after a one-character source edit', () => {
  withFixture(({ binDir, scriptsDir }) => {
    const idlePath = path.join(binDir, 'idle.ps1');
    const text = fs.readFileSync(idlePath, 'utf8');
    const mutated = text.replace(/(\$source = @"\r?\n)/, '$1// mutated-by-test\r\n');
    assert.notEqual(mutated, text, 'expected to find and mutate the $source block');
    fs.writeFileSync(idlePath, mutated);

    const { ok, stale } = checkAssemblies({ binDirectory: binDir, scriptsDirectory: scriptsDir });
    assert.equal(ok, false);
    assert.equal(stale.length, 1);
    assert.equal(stale[0].class, 'IdleLauncher');
    assert.match(stale[0].reason, /stale/);
  });
});

test('check-assemblies: detects a missing DLL', () => {
  withFixture(({ binDir, scriptsDir }) => {
    fs.rmSync(path.join(binDir, 'CapcLauncher.dll'));
    const { ok, stale } = checkAssemblies({ binDirectory: binDir, scriptsDirectory: scriptsDir });
    assert.equal(ok, false);
    assert.equal(stale.length, 1);
    assert.equal(stale[0].class, 'CapcLauncher');
    assert.match(stale[0].reason, /missing/);
  });
});

test('check-assemblies: detects a missing sidecar', () => {
  withFixture(({ binDir, scriptsDir }) => {
    fs.rmSync(path.join(binDir, 'HighLauncher.dll.managed'));
    const { ok, stale } = checkAssemblies({ binDirectory: binDir, scriptsDirectory: scriptsDir });
    assert.equal(ok, false);
    assert.equal(stale.length, 1);
    assert.equal(stale[0].class, 'HighLauncher');
    assert.match(stale[0].reason, /sidecar is missing/);
  });
});

test('check-assemblies: detects EnvironmentNotifier source drift too, not just launchers', () => {
  withFixture(({ binDir, scriptsDir }) => {
    const notifierPath = path.join(scriptsDir, 'EnvironmentNotifier.cs');
    fs.appendFileSync(notifierPath, '\n// mutated-by-test\n');
    const { ok, stale } = checkAssemblies({ binDirectory: binDir, scriptsDirectory: scriptsDir });
    assert.equal(ok, false);
    assert.equal(stale.length, 1);
    assert.equal(stale[0].class, 'EnvironmentNotifier');
    assert.match(stale[0].reason, /stale/);
  });
});
