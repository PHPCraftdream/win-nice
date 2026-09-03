'use strict';
const fs = require('fs');
const path = require('path');
const os = require('os');

const MARKER = '<!-- win-nice: managed-skill -->';
const SOURCE = path.join(__dirname, '..', 'skills', 'win-nice', 'SKILL.md');

// WIN_NICE_SKILL_HOME overrides where these live - used by tests, mirroring
// WIN_NICE_HOME for the bin/ installer.
function homeDir() {
  return process.env.WIN_NICE_SKILL_HOME || os.homedir();
}

function targets() {
  const home = homeDir();
  return [
    path.join(home, '.claude', 'skills', 'win-nice', 'SKILL.md'),
    // Not ~/.codex/skills - Codex CLI's current personal-skill location is
    // $HOME/.agents/skills (the open agentskills.io standard's user scope;
    // .codex/skills was an earlier/incorrect assumption, since corrected).
    path.join(home, '.agents', 'skills', 'win-nice', 'SKILL.md'),
  ];
}

// ~/.claude/skills and ~/.agents/skills are shared namespaces, not a directory
// win-nice owns exclusively (unlike %LOCALAPPDATA%\win-nice\bin for the bin/
// installer) - a "win-nice" folder there could belong to someone/something else
// entirely, so installing must never blindly overwrite an existing file.
function installSkill() {
  const content = fs.readFileSync(SOURCE, 'utf8');
  const results = [];
  for (const target of targets()) {
    if (fs.existsSync(target) && !fs.readFileSync(target, 'utf8').includes(MARKER)) {
      results.push({ file: target, installed: false, reason: 'already exists (not ours - refusing to overwrite)' });
      continue;
    }
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, content);
    results.push({ file: target, installed: true });
  }
  for (const r of results) {
    console.log(r.installed ? `installed skill: ${r.file}` : `skipped ${r.file} (${r.reason})`);
  }
  return results;
}

// Called from install() (postinstall / `win-nice install|reinstall`), not just
// the explicit opt-in `win-nice skill install` - a package upgrade must not
// leave a previously-installed skill copy silently stale (e.g. recommending
// tool names a breaking rename just deleted). Only refreshes copies that are
// ALREADY there and still carry the marker: never creates one for a user who
// never opted in, and never touches a missing or foreign/unmarked file.
function updateInstalledSkill() {
  const content = fs.readFileSync(SOURCE, 'utf8');
  const results = [];
  for (const target of targets()) {
    if (!fs.existsSync(target)) {
      results.push({ file: target, updated: false, reason: 'not installed' });
      continue;
    }
    if (!fs.readFileSync(target, 'utf8').includes(MARKER)) {
      results.push({ file: target, updated: false, reason: 'marker missing (modified by user?)' });
      continue;
    }
    fs.writeFileSync(target, content);
    results.push({ file: target, updated: true });
  }
  return results;
}

function uninstallSkill() {
  const results = [];
  for (const target of targets()) {
    if (!fs.existsSync(target)) {
      results.push({ file: target, removed: false, reason: 'missing' });
      continue;
    }
    if (!fs.readFileSync(target, 'utf8').includes(MARKER)) {
      results.push({ file: target, removed: false, reason: 'marker missing (modified by user?)' });
      continue;
    }
    fs.unlinkSync(target);
    results.push({ file: target, removed: true });
  }
  for (const r of results) {
    console.log(r.removed ? `removed ${r.file}` : `skipped ${r.file} (${r.reason})`);
  }
  return results;
}

module.exports = { installSkill, uninstallSkill, updateInstalledSkill, targets, MARKER };
