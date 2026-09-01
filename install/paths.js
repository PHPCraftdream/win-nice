'use strict';
const os = require('os');
const path = require('path');
const { execFileSync } = require('child_process');

// WIN_NICE_HOME overrides the install root - used by tests and by anyone who
// wants a non-default location. Real installs default to %LOCALAPPDATA%\win-nice.
function installRoot() {
  if (process.env.WIN_NICE_HOME) return process.env.WIN_NICE_HOME;
  const localAppData = process.env.LOCALAPPDATA || path.join(os.homedir(), 'AppData', 'Local');
  return path.join(localAppData, 'win-nice');
}

function binDir() {
  return path.join(installRoot(), 'bin');
}

function manifestPath() {
  return path.join(installRoot(), 'install-manifest.json');
}

function normalize(p) {
  return path.normalize(p).replace(/\\+$/, '').toLowerCase();
}

function addToPathString(currentPath, dir) {
  const parts = currentPath.split(';').filter(Boolean);
  const already = parts.some((p) => normalize(p) === normalize(dir));
  if (already) return currentPath;
  return [...parts, dir].join(';');
}

function removeFromPathString(currentPath, dir) {
  const parts = currentPath.split(';').filter(Boolean);
  return parts.filter((p) => normalize(p) !== normalize(dir)).join(';');
}

// Real registry reads/writes - via [Environment]::...('User') rather than `setx`,
// which truncates PATH silently past ~1024 chars. Not covered by unit tests;
// addToPathString/removeFromPathString carry the actual logic and are.
function readUserPath() {
  const out = execFileSync(
    'powershell',
    ['-NoProfile', '-Command', "[Environment]::GetEnvironmentVariable('Path','User')"],
    { encoding: 'utf8' }
  );
  return out.replace(/\r?\n$/, '');
}

function writeUserPath(newPath) {
  execFileSync(
    'powershell',
    ['-NoProfile', '-Command', "[Environment]::SetEnvironmentVariable('Path', $env:WIN_NICE_NEW_PATH, 'User')"],
    { env: { ...process.env, WIN_NICE_NEW_PATH: newPath } }
  );
}

module.exports = {
  installRoot,
  binDir,
  manifestPath,
  addToPathString,
  removeFromPathString,
  readUserPath,
  writeUserPath,
};
