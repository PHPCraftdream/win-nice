'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const { installSkill, uninstallSkill, targets, MARKER } = require('../install/skill');

function freshHome() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'win-nice-skillhome-'));
}

function withHome(home, fn) {
  const prev = process.env.WIN_NICE_SKILL_HOME;
  process.env.WIN_NICE_SKILL_HOME = home;
  try {
    return fn();
  } finally {
    if (prev === undefined) delete process.env.WIN_NICE_SKILL_HOME;
    else process.env.WIN_NICE_SKILL_HOME = prev;
    fs.rmSync(home, { recursive: true, force: true });
  }
}

test('installSkill copies SKILL.md into both ~/.claude/skills and ~/.codex/skills', () => {
  withHome(freshHome(), () => {
    installSkill();
    for (const target of targets()) {
      assert.ok(fs.existsSync(target), `${target} should exist`);
      const content = fs.readFileSync(target, 'utf8');
      assert.ok(content.includes(MARKER));
      assert.match(content, /^---/m);
      assert.match(content, /name: win-nice/);
    }
  });
});

test('uninstallSkill removes both files that install placed', () => {
  withHome(freshHome(), () => {
    installSkill();
    const results = uninstallSkill();
    assert.equal(results.length, 2);
    assert.ok(results.every((r) => r.removed));
    for (const target of targets()) assert.equal(fs.existsSync(target), false);
  });
});

test('uninstallSkill skips a file whose marker was stripped (user-modified)', () => {
  withHome(freshHome(), () => {
    installSkill();
    const [claudeTarget] = targets();
    fs.writeFileSync(claudeTarget, 'user replaced this file');

    const results = uninstallSkill();
    const claudeResult = results.find((r) => r.file === claudeTarget);
    assert.equal(claudeResult.removed, false);
    assert.equal(fs.existsSync(claudeTarget), true);
  });
});

test('uninstallSkill is a no-op when nothing is installed', () => {
  withHome(freshHome(), () => {
    const results = uninstallSkill();
    assert.ok(results.every((r) => r.removed === false && r.reason === 'missing'));
  });
});
