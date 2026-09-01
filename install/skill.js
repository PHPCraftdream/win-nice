'use strict';
const fs = require('fs');
const path = require('path');
const os = require('os');

const MARKER = '<!-- win-nice: managed-skill -->';
const SOURCE = path.join(__dirname, '..', 'skills', 'win-nice', 'SKILL.md');

// WIN_NICE_SKILL_HOME overrides where ~/.claude and ~/.codex are found - used by
// tests, mirroring WIN_NICE_HOME for the bin/ installer.
function homeDir() {
  return process.env.WIN_NICE_SKILL_HOME || os.homedir();
}

function targets() {
  const home = homeDir();
  return [
    path.join(home, '.claude', 'skills', 'win-nice', 'SKILL.md'),
    path.join(home, '.codex', 'skills', 'win-nice', 'SKILL.md'),
  ];
}

function installSkill() {
  const content = fs.readFileSync(SOURCE, 'utf8');
  for (const target of targets()) {
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, content);
    console.log(`installed skill: ${target}`);
  }
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

module.exports = { installSkill, uninstallSkill, targets, MARKER };
