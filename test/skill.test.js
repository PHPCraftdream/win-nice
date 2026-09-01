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

test('installSkill copies SKILL.md into both ~/.claude/skills and ~/.agents/skills', () => {
  withHome(freshHome(), () => {
    installSkill();
    for (const target of targets()) {
      assert.ok(fs.existsSync(target), `${target} should exist`);
      const content = fs.readFileSync(target, 'utf8');
      assert.ok(content.includes(MARKER));
      // YAML frontmatter parsers require "---" to be the file's literal first
      // content - a preceding marker comment (the original bug) would break this.
      assert.ok(content.startsWith('---'), 'frontmatter must be the first content in the file');
      assert.match(content, /name: win-nice/);
    }
  });
});

test('targets() points at ~/.agents/skills for Codex, not ~/.codex/skills', () => {
  withHome(freshHome(), () => {
    const agentsTarget = targets().find((t) => t.split(path.sep).includes('.agents'));
    assert.ok(agentsTarget, 'expected a target under .agents');
    assert.ok(!targets().some((t) => t.split(path.sep).includes('.codex')), 'no target should use .codex');
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

test('installSkill refuses to overwrite a pre-existing unrelated file with the same name', () => {
  withHome(freshHome(), () => {
    const [claudeTarget] = targets();
    fs.mkdirSync(path.dirname(claudeTarget), { recursive: true });
    fs.writeFileSync(claudeTarget, 'someone else\'s unrelated win-nice skill');

    const results = installSkill();
    const claudeResult = results.find((r) => r.file === claudeTarget);
    assert.equal(claudeResult.installed, false);
    assert.equal(fs.readFileSync(claudeTarget, 'utf8'), 'someone else\'s unrelated win-nice skill');
  });
});

test('installSkill overwrites its own previously-installed (marked) copy', () => {
  withHome(freshHome(), () => {
    installSkill();
    const [claudeTarget] = targets();
    const results = installSkill();
    assert.ok(results.every((r) => r.installed));
    assert.ok(fs.readFileSync(claudeTarget, 'utf8').includes(MARKER));
  });
});
