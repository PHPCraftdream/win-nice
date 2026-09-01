'use strict';
const fs = require('fs');
const path = require('path');
const paths = require('./paths');
const manifest = require('./manifest');
const { removeManagedFile } = require('./uninstall');
const pkg = require('../package.json');

const SOURCE_BIN = path.join(__dirname, '..', 'bin');

function listSourceFiles() {
  // .bat/.ps1 launchers plus their extensionless POSIX shell shims (bin/<tool>,
  // no dot) - the sibling Git Bash needs since it ignores PATHEXT on bare names.
  return fs.readdirSync(SOURCE_BIN).filter((f) => f.endsWith('.bat') || f.endsWith('.ps1') || !f.includes('.'));
}

// Guards against `npm install`/`npm test` inside a source checkout silently
// touching the real system PATH - only a genuine package install (running from
// inside someone's node_modules) or an explicit WIN_NICE_HOME override proceeds.
function isSourceCheckout() {
  return fs.existsSync(path.join(__dirname, '..', '.git'));
}

// `npm install -g win-nice@newer` only runs postinstall (this function) - unlike
// `win-nice reinstall`, which does uninstall()+install(), it never diffs against
// what a previous version left behind. Without this, a tool dropped in a newer
// version stays orphaned in binDir forever. Safe to call before the target dir
// even exists (read() returns null, staleNames is empty).
function cleanupStaleFiles(dir, currentFiles) {
  const previous = manifest.read(paths.manifestPath());
  if (!previous || !Array.isArray(previous.files)) return;

  const currentSet = new Set(currentFiles);
  const staleNames = previous.files.filter((name) => !currentSet.has(name));
  const previousDir = previous.binDir || dir;
  for (const name of staleNames) {
    removeManagedFile(path.join(previousDir, name), dir, { requireMarker: false });
  }
}

function install({ updatePath = true } = {}) {
  if (!process.env.WIN_NICE_HOME && isSourceCheckout()) {
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
  return { dir, files };
}

module.exports = { install, listSourceFiles, isSourceCheckout };
