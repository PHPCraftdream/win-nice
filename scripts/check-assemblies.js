#!/usr/bin/env node
'use strict';
// Freshness gate for the 14 prebuilt helper assemblies, without a compiler
// (P2-1). scripts/build-launcher-assemblies.ps1 hashes each launcher's
// extracted C# source (or, for EnvironmentNotifier, the whole .cs file) into
// its bin/<Class>.dll.managed sidecar as a `# source-sha256: <hex>` line.
// This recomputes the same hash - same extraction regex, same CRLF-to-LF
// normalization, same recipe prefix - and fails if a sidecar's hash doesn't
// match its current source, or if a DLL/sidecar is plain missing. It never
// invokes csc.exe, so it can run on every `npm test`/`npm pack` without
// rewriting tracked binaries.
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const { HELPER_ASSEMBLIES } = require('../install/install');

const repoRoot = path.join(__dirname, '..');
const defaultBinDir = path.join(repoRoot, 'bin');
const defaultScriptsDir = path.join(repoRoot, 'scripts');

// Must stay byte-for-byte identical to build-launcher-assemblies.ps1's
// $buildRecipe - any part of it changing is a deliberate hash-busting bump.
const BUILD_RECIPE = 'win-nice-helper-v1|csc4|/target:library|/reference:System.dll\n';
const SOURCE_BLOCK_PATTERN = /\$source = @"\r?\n([\s\S]*?)\r?\n"@/;
const SIDECAR_HASH_PATTERN = /^# source-sha256: ([0-9a-f]{64})$/m;
const MARKER = 'win-nice: managed-file';

function sourceHash(sourceText) {
  const normalized = sourceText.replace(/\r\n/g, '\n');
  return crypto.createHash('sha256').update(BUILD_RECIPE + normalized, 'utf8').digest('hex');
}

function extractLauncherSource(scriptPath) {
  const text = fs.readFileSync(scriptPath, 'utf8');
  const match = SOURCE_BLOCK_PATTERN.exec(text);
  if (!match) throw new Error(`could not find a $source block in ${scriptPath}`);
  return match[1];
}

// One assembly's result: { class, ok, reason? }. binDirectory/scriptsDir are
// overridable so the fixture test below can point this at a temp copy
// instead of the real repo.
function checkOne(assembly, { binDirectory, scriptsDirectory }) {
  const { class: className, script } = assembly;
  let expectedHash;
  try {
    expectedHash = script
      ? sourceHash(extractLauncherSource(path.join(binDirectory, script)))
      : sourceHash(fs.readFileSync(path.join(scriptsDirectory, 'EnvironmentNotifier.cs'), 'utf8'));
  } catch (err) {
    return { class: className, ok: false, reason: `cannot read source: ${err.message}` };
  }

  const dllPath = path.join(binDirectory, `${className}.dll`);
  const sidecarPath = `${dllPath}.managed`;
  if (!fs.existsSync(dllPath)) {
    return { class: className, ok: false, reason: `${className}.dll is missing` };
  }

  let sidecarText;
  try {
    sidecarText = fs.readFileSync(sidecarPath, 'utf8');
  } catch {
    return { class: className, ok: false, reason: `${className}.dll.managed sidecar is missing` };
  }
  if (!sidecarText.includes(MARKER)) {
    return { class: className, ok: false, reason: `${className}.dll.managed is missing the managed-file marker` };
  }
  const hashMatch = SIDECAR_HASH_PATTERN.exec(sidecarText);
  if (!hashMatch) {
    return { class: className, ok: false, reason: `${className}.dll.managed has no source-sha256 line (stale sidecar format)` };
  }
  if (hashMatch[1] !== expectedHash) {
    return { class: className, ok: false, reason: `${className}.dll is stale (source changed since last build)` };
  }
  return { class: className, ok: true };
}

function checkAssemblies({ binDirectory = defaultBinDir, scriptsDirectory = defaultScriptsDir } = {}) {
  const results = HELPER_ASSEMBLIES.map((a) => checkOne(a, { binDirectory, scriptsDirectory }));
  const stale = results.filter((r) => !r.ok);
  return { ok: stale.length === 0, results, stale };
}

function main() {
  const { ok, stale } = checkAssemblies();
  if (ok) {
    // stderr, not stdout: this runs as `prepack`, and `npm pack --json`
    // shares this process's stdout with npm's own JSON output - anything
    // this script prints to stdout would corrupt that JSON for callers
    // (release-check.js, test/release-check-allowlist.test.js) parsing it.
    console.error(`check-assemblies: ok - all ${HELPER_ASSEMBLIES.length} helper assemblies are up to date`);
    return;
  }
  console.error('check-assemblies: FAIL - stale/missing helper assembl' + (stale.length === 1 ? 'y' : 'ies') + ':');
  for (const r of stale) {
    console.error(`  - ${r.class}: ${r.reason}`);
  }
  console.error('Fix: run scripts/build-launcher-assemblies.ps1 and commit bin/*.dll + bin/*.dll.managed');
  process.exitCode = 1;
}

if (require.main === module) main();

module.exports = { checkAssemblies, sourceHash, BUILD_RECIPE, extractLauncherSource };
