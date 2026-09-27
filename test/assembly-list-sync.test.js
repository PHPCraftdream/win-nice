'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const { HELPER_ASSEMBLIES } = require('../install/install');

const buildScriptPath = path.join(__dirname, '..', 'scripts', 'build-launcher-assemblies.ps1');

// P3-3: install/install.js's HELPER_ASSEMBLIES is the single source of truth
// for the script<->class mapping. build-launcher-assemblies.ps1 keeps its own
// $launcherSources table (PowerShell can't require a Node module), so this
// pins the two in sync instead - a launcher added to one without the other
// fails here.
test('build-launcher-assemblies.ps1 $launcherSources matches install/install.js HELPER_ASSEMBLIES', () => {
  const source = fs.readFileSync(buildScriptPath, 'utf8');
  const entryPattern = /@\{\s*Script\s*=\s*'([^']+)';\s*Class\s*=\s*'([^']+)'\s*\}/g;
  const entries = [];
  let match;
  while ((match = entryPattern.exec(source))) {
    entries.push({ script: match[1], class: match[2] });
  }

  const expectedLaunchers = HELPER_ASSEMBLIES
    .filter((a) => a.script !== null)
    .map((a) => ({ script: a.script, class: a.class }));

  assert.ok(entries.length > 0, 'expected to find at least one $launcherSources entry');
  assert.deepEqual(entries, expectedLaunchers);
});

test('build-launcher-assemblies.ps1 still builds EnvironmentNotifier separately from scripts/EnvironmentNotifier.cs', () => {
  const source = fs.readFileSync(buildScriptPath, 'utf8');
  assert.match(source, /EnvironmentNotifier\.cs/);
  assert.match(source, /EnvironmentNotifier\.dll/);

  const notifierEntry = HELPER_ASSEMBLIES.find((a) => a.class === 'EnvironmentNotifier');
  assert.ok(notifierEntry, 'HELPER_ASSEMBLIES must include an EnvironmentNotifier entry');
  assert.equal(notifierEntry.script, null, 'EnvironmentNotifier has no launcher .ps1 of its own');
});
