'use strict';
// Shared isolation helpers for the installer tests.
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { EXE_NAME } = require('../../install/install');

// Fake prebuilt binary: install() reads its source exe from WIN_NICE_SOURCE_BIN
// so tests never need the real Rust-built bin/wnice.exe.
//
// Empirical fact (verified on this machine): Node's fs.openSync(p, 'r') does
// NOT block unlink or copyFileSync over the file, and rename always succeeds -
// libuv opens handles with FILE_SHARE_DELETE on Windows. Therefore a real
// running-exe image lock CANNOT be simulated with a plain Node handle; the
// tests below inject failures through the `_fs` seam (install) or by
// monkey-patching fs (uninstall) instead.
const FAKE_EXE = Buffer.from('fake wnexe');

function freshHome() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'win-nice-home-'));
}

function makeSourceBin() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'win-nice-srcbin-'));
  fs.writeFileSync(path.join(dir, EXE_NAME), FAKE_EXE);
  return dir;
}

// updatePath is always false in these tests - they must never touch the real
// user PATH/registry. That mutation path is exercised manually, not here.
//
// install() now also calls updateInstalledSkill() internally (see
// install/skill.js), which reads its target directory from the SEPARATE
// WIN_NICE_SKILL_HOME env var, not WIN_NICE_HOME - so isolating only
// WIN_NICE_HOME here would leave every install() call in this file free to
// read/write the real ~/.claude/skills and ~/.agents/skills (confirmed: it
// did, once, before this env var was added here). Isolate both.
function withHome(home, fn) {
  const prev = process.env.WIN_NICE_HOME;
  const prevSkillHome = process.env.WIN_NICE_SKILL_HOME;
  const prevSourceBin = process.env.WIN_NICE_SOURCE_BIN;
  const skillHome = fs.mkdtempSync(path.join(os.tmpdir(), 'win-nice-skillhome-'));
  const sourceBin = makeSourceBin();
  process.env.WIN_NICE_HOME = home;
  process.env.WIN_NICE_SKILL_HOME = skillHome;
  process.env.WIN_NICE_SOURCE_BIN = sourceBin;
  try {
    return fn();
  } finally {
    if (prev === undefined) delete process.env.WIN_NICE_HOME;
    else process.env.WIN_NICE_HOME = prev;
    if (prevSkillHome === undefined) delete process.env.WIN_NICE_SKILL_HOME;
    else process.env.WIN_NICE_SKILL_HOME = prevSkillHome;
    if (prevSourceBin === undefined) delete process.env.WIN_NICE_SOURCE_BIN;
    else process.env.WIN_NICE_SOURCE_BIN = prevSourceBin;
    fs.rmSync(home, { recursive: true, force: true });
    fs.rmSync(skillHome, { recursive: true, force: true });
    fs.rmSync(sourceBin, { recursive: true, force: true });
  }
}

module.exports = { FAKE_EXE, freshHome, withHome };
