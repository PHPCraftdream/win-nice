'use strict';
const fs = require('fs');
const path = require('path');
const paths = require('./paths');
const manifest = require('./manifest');

function removeManagedFile(filePath) {
  if (!fs.existsSync(filePath)) return { file: filePath, removed: false, reason: 'missing' };
  if (!manifest.hasMarker(filePath)) {
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
  if (data && Array.isArray(data.files)) {
    candidates = data.files.map((name) => path.join(data.binDir || dir, name));
  } else if (fs.existsSync(dir)) {
    // Manifest missing/corrupt - fall back to scanning the known install dir,
    // scoped to that one directory, only removing files that carry the marker.
    candidates = fs.readdirSync(dir).map((name) => path.join(dir, name));
  } else {
    candidates = [];
  }

  const results = candidates.map(removeManagedFile);
  for (const r of results) {
    console.log(r.removed ? `removed ${r.file}` : `skipped ${r.file} (${r.reason})`);
  }

  if (fs.existsSync(dir) && fs.readdirSync(dir).length === 0) fs.rmdirSync(dir);
  if (fs.existsSync(manifestFile)) fs.unlinkSync(manifestFile);

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
