'use strict';
const fs = require('fs');
const path = require('path');
const paths = require('./paths');
const manifest = require('./manifest');

// Rejects any candidate that doesn't resolve inside expectedDir - guards against a
// corrupted/tampered manifest (absolute paths, "../.." traversal) pointing deletion
// outside the install directory.
function isInsideDir(candidate, expectedDir) {
  const relative = path.relative(path.resolve(expectedDir), path.resolve(candidate));
  return relative !== '' && relative !== '..' && !relative.startsWith('..' + path.sep) && !path.isAbsolute(relative);
}

function removeManagedFile(filePath, expectedDir, { requireMarker }) {
  if (!isInsideDir(filePath, expectedDir)) {
    return { file: filePath, removed: false, reason: 'rejected (escapes install directory)' };
  }
  if (!fs.existsSync(filePath)) return { file: filePath, removed: false, reason: 'missing' };
  if (requireMarker && !manifest.hasMarker(filePath)) {
    return { file: filePath, removed: false, reason: 'marker missing (modified by user?)' };
  }
  fs.unlinkSync(filePath);
  return { file: filePath, removed: true };
}

function uninstall({ updatePath = true } = {}) {
  const dir = paths.binDir();
  const manifestFile = paths.manifestPath();
  const data = manifest.read(manifestFile);

  let candidates;
  let requireMarker;
  if (data && Array.isArray(data.files)) {
    // Files tracked by a valid manifest are owned by the package - remove them
    // regardless of local edits (reinstall is expected to replace them anyway).
    candidates = data.files.map((name) => path.join(data.binDir || dir, name));
    requireMarker = false;
  } else if (fs.existsSync(dir)) {
    // Manifest missing/corrupt - fall back to scanning the known install dir. This
    // scan can hit files we didn't put there, so only remove ones that still carry
    // the marker.
    candidates = fs.readdirSync(dir).map((name) => path.join(dir, name));
    requireMarker = true;
  } else {
    candidates = [];
    requireMarker = true;
  }

  const results = candidates.map((candidate) => removeManagedFile(candidate, dir, { requireMarker }));
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

module.exports = { uninstall, removeManagedFile };
