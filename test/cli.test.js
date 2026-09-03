'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const CLI = path.join(__dirname, '..', 'install', 'cli.js');

function run(args, home, skillHome, extraEnv) {
  return spawnSync(process.execPath, [CLI, ...args], {
    encoding: 'utf8',
    env: { ...process.env, WIN_NICE_HOME: home, WIN_NICE_SKILL_HOME: skillHome, WIN_NICE_NO_PATH: '1', ...extraEnv },
  });
}

// install() (called by the CLI's `install`/`reinstall`) also calls
// updateInstalledSkill() internally, which resolves its targets from the
// SEPARATE WIN_NICE_SKILL_HOME env var, not WIN_NICE_HOME - so a home() here
// that only isolated WIN_NICE_HOME left every run() call in this file free to
// read/write the real ~/.claude/skills and ~/.agents/skills (confirmed: it
// did, once, before this second temp dir was added). Isolate both, same as
// test/install-uninstall.test.js's withHome().
function withHome(fn) {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'win-nice-cli-'));
  const skillHome = fs.mkdtempSync(path.join(os.tmpdir(), 'win-nice-cli-skillhome-'));
  try {
    fn(home, skillHome);
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
    fs.rmSync(skillHome, { recursive: true, force: true });
  }
}

function runSkill(args, skillHome) {
  return spawnSync(process.execPath, [CLI, 'skill', ...args], {
    encoding: 'utf8',
    env: { ...process.env, WIN_NICE_SKILL_HOME: skillHome },
  });
}

function withSkillHome(fn) {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'win-nice-cli-skillhome-'));
  try {
    fn(home);
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
}

test('cli status reports "not installed" against a fresh home', () => {
  withHome((home, skillHome) => {
    const res = run(['status'], home, skillHome);
    assert.equal(res.status, 0);
    assert.match(res.stdout, /not installed/);
  });
});

test('cli install then status reports the installed version and files', () => {
  withHome((home, skillHome) => {
    const installResult = run(['install'], home, skillHome);
    assert.equal(installResult.status, 0, installResult.stderr);

    const res = run(['status'], home, skillHome);
    assert.match(res.stdout, /win-nice .* installed/);
    assert.match(res.stdout, /idle\.bat/);
  });
});

test('cli uninstall removes a prior install', () => {
  withHome((home, skillHome) => {
    run(['install'], home, skillHome);
    const uninstallResult = run(['uninstall'], home, skillHome);
    assert.equal(uninstallResult.status, 0, uninstallResult.stderr);

    const res = run(['status'], home, skillHome);
    assert.match(res.stdout, /not installed/);
  });
});

test('cli rejects an unknown command with a non-zero exit code', () => {
  withHome((home, skillHome) => {
    const res = run(['bogus'], home, skillHome);
    assert.notEqual(res.status, 0);
    assert.match(res.stderr, /unknown command/);
  });
});

test('cli status tolerates a manifest whose "files" field is not an array', () => {
  withHome((home, skillHome) => {
    run(['install'], home, skillHome);
    const manifestFile = path.join(home, 'install-manifest.json');
    const data = JSON.parse(fs.readFileSync(manifestFile, 'utf8'));
    data.files = null;
    fs.writeFileSync(manifestFile, JSON.stringify(data));

    const res = run(['status'], home, skillHome);
    assert.equal(res.status, 0, res.stderr);
    assert.match(res.stdout, /malformed/);
  });
});

test('cli skill install places SKILL.md under both .claude and .agents skills dirs', () => {
  withSkillHome((home) => {
    const res = runSkill(['install'], home);
    assert.equal(res.status, 0, res.stderr);
    assert.ok(fs.existsSync(path.join(home, '.claude', 'skills', 'win-nice', 'SKILL.md')));
    assert.ok(fs.existsSync(path.join(home, '.agents', 'skills', 'win-nice', 'SKILL.md')));
  });
});

test('cli skill uninstall removes what skill install placed', () => {
  withSkillHome((home) => {
    runSkill(['install'], home);
    const res = runSkill(['uninstall'], home);
    assert.equal(res.status, 0, res.stderr);
    assert.equal(fs.existsSync(path.join(home, '.claude', 'skills', 'win-nice', 'SKILL.md')), false);
    assert.equal(fs.existsSync(path.join(home, '.agents', 'skills', 'win-nice', 'SKILL.md')), false);
  });
});

test('cli rejects an unknown skill subcommand', () => {
  withSkillHome((home) => {
    const res = runSkill(['bogus'], home);
    assert.notEqual(res.status, 0);
    assert.match(res.stderr, /unknown skill command/);
  });
});

test('cli skill install exits nonzero when a target is skipped (foreign file present)', () => {
  withSkillHome((home) => {
    const claudeTarget = path.join(home, '.claude', 'skills', 'win-nice', 'SKILL.md');
    fs.mkdirSync(path.dirname(claudeTarget), { recursive: true });
    fs.writeFileSync(claudeTarget, 'not ours');

    const res = runSkill(['install'], home);
    assert.notEqual(res.status, 0);
    assert.match(res.stdout, /skipped/);
  });
});

test('cli skill install exits 0 on a clean install with no conflicts', () => {
  withSkillHome((home) => {
    const res = runSkill(['install'], home);
    assert.equal(res.status, 0, res.stderr);
  });
});

test('cli skill uninstall exits nonzero when a target was modified by the user (marker missing)', () => {
  withSkillHome((home) => {
    runSkill(['install'], home);
    const claudeTarget = path.join(home, '.claude', 'skills', 'win-nice', 'SKILL.md');
    fs.writeFileSync(claudeTarget, 'user replaced this file');

    const res = runSkill(['uninstall'], home);
    assert.notEqual(res.status, 0);
    assert.match(res.stdout, /skipped/);
  });
});

test('cli skill uninstall exits 0 for a normal uninstall of a clean install', () => {
  withSkillHome((home) => {
    runSkill(['install'], home);
    const res = runSkill(['uninstall'], home);
    assert.equal(res.status, 0, res.stderr);
  });
});

// Regression for the bug this file's run() helper actually had: install()
// calls updateInstalledSkill() internally, which resolves its targets via
// WIN_NICE_SKILL_HOME - or, if that's unset, os.homedir(). run() used to set
// only WIN_NICE_HOME, so every `run(['install'], home)` call in this file was
// silently free to rewrite the real developer's ~/.claude/skills and
// ~/.agents/skills. On Windows, os.homedir() resolves via the USERPROFILE
// env var, which lets this test detect (and prove the fix for) that leak
// without ever touching the real user home: it points USERPROFILE/HOME at a
// throwaway temp dir for the duration of two child processes only.
test('regression: install() must never fall through to the ambient home when WIN_NICE_SKILL_HOME is set', () => {
  withHome((home, skillHome) => {
    const ambientHome = fs.mkdtempSync(path.join(os.tmpdir(), 'win-nice-ambient-home-'));
    try {
      const sentinelTarget = path.join(ambientHome, '.claude', 'skills', 'win-nice', 'SKILL.md');
      fs.mkdirSync(path.dirname(sentinelTarget), { recursive: true });
      const sentinelContent = '<!-- win-nice: managed-skill -->\nSENTINEL - must not change\n';

      // Mechanism sanity check: reproduce the historical bug on purpose by
      // omitting WIN_NICE_SKILL_HOME - this must leak into ambientHome (via
      // the overridden USERPROFILE/HOME), confirming the detector works.
      fs.writeFileSync(sentinelTarget, sentinelContent);
      const buggy = spawnSync(process.execPath, [CLI, 'install'], {
        encoding: 'utf8',
        env: { ...process.env, WIN_NICE_HOME: home, WIN_NICE_NO_PATH: '1', USERPROFILE: ambientHome, HOME: ambientHome },
      });
      assert.equal(buggy.status, 0, buggy.stderr);
      assert.notEqual(
        fs.readFileSync(sentinelTarget, 'utf8'),
        sentinelContent,
        'sanity check failed: omitting WIN_NICE_SKILL_HOME should reproduce the ambient-home leak'
      );

      // The actual regression check: this file's own run()/withHome() helpers
      // must isolate WIN_NICE_SKILL_HOME and leave the ambient home alone.
      fs.writeFileSync(sentinelTarget, sentinelContent);
      const res = run(['install'], home, skillHome, { USERPROFILE: ambientHome, HOME: ambientHome });
      assert.equal(res.status, 0, res.stderr);
      assert.equal(
        fs.readFileSync(sentinelTarget, 'utf8'),
        sentinelContent,
        'run() must never touch the ambient home once WIN_NICE_SKILL_HOME is set'
      );
    } finally {
      fs.rmSync(ambientHome, { recursive: true, force: true });
    }
  });
});
