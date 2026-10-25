'use strict';
const fs = require('fs');
const path = require('path');
const paths = require('./paths');
const manifest = require('./manifest');

function orderManagedFiles(names) {
  // Exes/sidecars first, then shims: deleting a shim whose target exe is
  // already gone is harmless, but the reverse would briefly leave dangling
  // names on PATH.
  const binaries = names.filter((n) => n.endsWith('.exe') || n.endsWith('.managed'));
  const rest = names.filter((n) => !binaries.includes(n));
  return [...binaries, ...rest];
}

// Rejects any candidate that doesn't resolve inside expectedDir - guards against a
// corrupted/tampered manifest (absolute paths, "../.." traversal) pointing deletion
// outside the install directory.
function isInsideDir(candidate, expectedDir) {
  const relative = path.relative(path.resolve(expectedDir), path.resolve(candidate));
  return relative !== '' && relative !== '..' && !relative.startsWith('..' + path.sep) && !path.isAbsolute(relative);
}

function removeManagedFile(filePath, expectedDir, { requireOwnership }) {
  if (!isInsideDir(filePath, expectedDir)) {
    return { file: filePath, removed: false, reason: 'rejected (escapes install directory)' };
  }
  if (!fs.existsSync(filePath)) return { file: filePath, removed: false, reason: 'missing' };
  // Lazy require: install.js requires this module, so a top-level require back
  // would hit the circular-dependency partial-exports problem.
  const { isOwnedArtifact } = require('./install');
  if (requireOwnership && !isOwnedArtifact(filePath, expectedDir)) {
    return { file: filePath, removed: false, reason: 'not a win-nice-managed file (modified by user?)' };
  }
  try {
    fs.unlinkSync(filePath);
    return { file: filePath, removed: true };
  } catch (err) {
    // Never throw: a stuck (running) exe would otherwise abort the whole
    // uninstall. Both manifest-tracked files (requireOwnership === false) and
    // fallback-verified-owned ones are certainly ours, so try to move them
    // aside like install()'s Phase R does - a running exe can't be deleted on
    // Windows, but it CAN be renamed.
    try {
      const { renameAside } = require('./install');
      const aside = renameAside(filePath);
      if (aside) {
        return { file: filePath, removed: false, reason: `in use - renamed aside to ${path.basename(aside)}` };
      }
    } catch {}
    return { file: filePath, removed: false, reason: `in use and cannot be renamed (${err && err.code})` };
  }
}

function uninstall({ updatePath = true } = {}) {
  // Mirrors install()'s own guard: without it, `win-nice reinstall`/`uninstall`
  // run from inside a git clone (no WIN_NICE_HOME) would delete a real prior
  // install and PATH entry, then (for reinstall) silently skip reinstalling it.
  if (!process.env.WIN_NICE_HOME && paths.isSourceCheckout()) {
    console.log(
      'Running from a source checkout - skipping real uninstall. ' +
        'Set WIN_NICE_HOME to force a target directory, or uninstall the published package.'
    );
    return [];
  }

  const dir = paths.binDir();
  const manifestFile = paths.manifestPath();
  const data = manifest.read(manifestFile);

  let candidates;
  let requireOwnership;
  if (data && Array.isArray(data.files)) {
    // Files tracked by a valid manifest are owned by the package - remove them
    // regardless of local edits (reinstall is expected to replace them anyway).
    candidates = orderManagedFiles(data.files).map((name) => path.join(data.binDir || dir, name));
    requireOwnership = false;
  } else if (fs.existsSync(dir)) {
    // Manifest missing/corrupt - fall back to scanning the known install dir. This
    // scan can hit files we didn't put there, so only remove ones we can prove
    // are ours.
    candidates = orderManagedFiles(fs.readdirSync(dir)).map((name) => path.join(dir, name));
    requireOwnership = true;
  } else {
    candidates = [];
    requireOwnership = true;
  }

  const results = candidates.map((candidate) => removeManagedFile(candidate, dir, { requireOwnership }));

  // Read AFTER the candidate deletions above, so any .old-N files this very
  // run created via removeManagedFile's rename-aside are included. A dir left
  // holding only stuck .old-N files (still-running old exe) stays behind: the
  // rmdir below correctly skips non-empty dirs - acceptable, the next
  // install()/uninstall() sweeps them.
  if (fs.existsSync(dir)) {
    const { isOldLeftoverName } = require('./install');
    for (const entry of fs.readdirSync(dir)) {
      if (!isOldLeftoverName(entry)) continue;
      const filePath = path.join(dir, entry);
      try {
        fs.unlinkSync(filePath);
        results.push({ file: filePath, removed: true, reason: 'leftover' });
      } catch (err) {
        results.push({ file: filePath, removed: false, reason: `in use and cannot be renamed (${err && err.code})` });
      }
    }
  }

  for (const r of results) {
    console.log(r.removed ? `removed ${r.file}` : `skipped ${r.file} (${r.reason})`);
  }

  if (fs.existsSync(dir) && fs.readdirSync(dir).length === 0) fs.rmdirSync(dir);
  if (fs.existsSync(manifestFile)) fs.unlinkSync(manifestFile);

  // Leaves %LOCALAPPDATA%\win-nice itself behind otherwise - only bin/ and the
  // manifest were ever tracked. Only remove it if uninstall left it truly empty;
  // a user-added file there must survive.
  const root = paths.installRoot();
  if (fs.existsSync(root) && fs.readdirSync(root).length === 0) fs.rmdirSync(root);

  if (updatePath) {
    const current = paths.readUserPath();
    const next = paths.removeFromPathString(current, dir);
    if (next !== current) {
      paths.writeUserPath(next);
      console.log(`Removed ${dir} from your PATH.`);
    }
  }

  return results;
}

module.exports = { uninstall, removeManagedFile, orderManagedFiles };
