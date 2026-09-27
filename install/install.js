'use strict';
const fs = require('fs');
const path = require('path');
const paths = require('./paths');
const manifest = require('./manifest');
const { removeManagedFile, orderManagedFiles } = require('./uninstall');
const { updateInstalledSkill } = require('./skill');
const pkg = require('../package.json');

const SOURCE_BIN = path.join(__dirname, '..', 'bin');

// Single source of truth for the 14 prebuilt helper assemblies (P3-3): the
// script<->class mapping scripts/build-launcher-assemblies.ps1 and
// scripts/check-assemblies.js both need to extract/verify each launcher's
// embedded C# source, plus the flat name list release-check.js and this
// file's own SUPPORT_FILES use. EnvironmentNotifier has no launcher .ps1 of
// its own (its source is scripts/EnvironmentNotifier.cs directly), hence
// `script: null`.
const HELPER_ASSEMBLIES = [
  { class: 'AboveNormalLauncher', script: 'abovenormal.ps1' },
  { class: 'AdminLauncher', script: 'admin.ps1' },
  { class: 'BelowNormalLauncher', script: 'belownormal.ps1' },
  { class: 'CapcLauncher', script: 'capc.ps1' },
  { class: 'CapmLauncher', script: 'capm.ps1' },
  { class: 'CapnLauncher', script: 'capn.ps1' },
  { class: 'CapsLauncher', script: 'caps.ps1' },
  { class: 'CaptLauncher', script: 'capt.ps1' },
  { class: 'CxLauncher', script: 'cx.ps1' },
  { class: 'CyLauncher', script: 'cy.ps1' },
  { class: 'HighLauncher', script: 'high.ps1' },
  { class: 'IdleLauncher', script: 'idle.ps1' },
  { class: 'RealtimeLauncher', script: 'realtime.ps1' },
  { class: 'EnvironmentNotifier', script: null },
];
const HELPER_ASSEMBLY_NAMES = HELPER_ASSEMBLIES.map((a) => a.class);
const SUPPORT_FILES = HELPER_ASSEMBLY_NAMES.flatMap((name) => [`${name}.dll`, `${name}.dll.managed`]);

function listSourceFiles() {
  // .bat/.ps1 launchers plus their extensionless POSIX shell shims (bin/<tool>,
  // no dot) - the sibling Git Bash needs since it ignores PATHEXT on bare names.
  const launchers = fs.readdirSync(SOURCE_BIN)
    .filter((f) => f.endsWith('.bat') || f.endsWith('.ps1') || !f.includes('.'));
  return [...launchers, ...SUPPORT_FILES];
}

// `npm install -g win-nice@newer` only runs postinstall (this function) - unlike
// `win-nice reinstall`, which does uninstall()+install(), it never diffs against
// what a previous version left behind. Without this, a tool dropped in a newer
// version stays orphaned in binDir forever. Safe to call before the target dir
// even exists (read() returns null, staleNames is empty).
function cleanupStaleFiles(dir, currentFiles) {
  const currentSet = new Set(currentFiles);
  const previous = manifest.read(paths.manifestPath());
  if (previous && Array.isArray(previous.files)) {
    const staleNames = orderManagedFiles(previous.files.filter((name) => !currentSet.has(name)));
    const previousDir = previous.binDir || dir;
    for (const name of staleNames) {
      removeManagedFile(path.join(previousDir, name), dir, { requireMarker: false });
    }
    return;
  }

  // Manifest missing/corrupt (deleted by hand, or predates this file's
  // existence) - same recovery uninstall() already relies on for the same
  // situation: scan the known install dir directly and remove only entries
  // that both (a) aren't part of the CURRENT tool set and (b) still carry the
  // "win-nice: managed-file" marker. An unmarked file (something the user
  // dropped into binDir themselves) is never touched, marker or no manifest.
  if (!fs.existsSync(dir)) return;
  for (const name of orderManagedFiles(fs.readdirSync(dir))) {
    if (currentSet.has(name)) continue;
    removeManagedFile(path.join(dir, name), dir, { requireMarker: true });
  }
}

function install({ updatePath = true } = {}) {
  if (!process.env.WIN_NICE_HOME && paths.isSourceCheckout()) {
    console.log(
      'Running from a source checkout - skipping real install. ' +
        'Set WIN_NICE_HOME to force a target directory, or install the published package.'
    );
    return null;
  }

  const dir = paths.binDir();
  fs.mkdirSync(dir, { recursive: true });

  const files = listSourceFiles();
  cleanupStaleFiles(dir, files);
  for (const name of files) {
    fs.copyFileSync(path.join(SOURCE_BIN, name), path.join(dir, name));
  }

  manifest.write(paths.manifestPath(), {
    version: pkg.version,
    installedAt: new Date().toISOString(),
    binDir: dir,
    files,
  });

  if (updatePath) {
    const current = paths.readUserPath();
    const next = paths.addToPathString(current, dir);
    if (next !== current) {
      paths.writeUserPath(next);
      console.log(`Added ${dir} to your PATH. Restart your terminal for it to take effect.`);
    }
  }

  console.log(`win-nice ${pkg.version} installed: ${files.join(', ')} -> ${dir}`);

  // Only touches a skill copy that's already there and still ours (opt-in
  // stays opt-in) - see updateInstalledSkill's own comment for why this needs
  // to run on every install(), not just the explicit `win-nice skill install`.
  for (const r of updateInstalledSkill()) {
    if (r.updated) console.log(`updated skill: ${r.file}`);
  }

  return { dir, files };
}

module.exports = {
  install,
  listSourceFiles,
  isSourceCheckout: paths.isSourceCheckout,
  HELPER_ASSEMBLIES,
  HELPER_ASSEMBLY_NAMES,
  SUPPORT_FILES,
};
