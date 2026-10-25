#!/usr/bin/env node
'use strict';
const { install } = require('./install');
const { uninstall } = require('./uninstall');
const { installSkill, uninstallSkill } = require('./skill');
const paths = require('./paths');
const manifest = require('./manifest');

function status() {
  const data = manifest.read(paths.manifestPath());
  if (!data) {
    console.log('win-nice is not installed.');
    return;
  }
  console.log(`win-nice ${data.version} installed at ${data.installedAt}`);
  console.log(`bin dir: ${data.binDir}`);
  console.log(`files: ${Array.isArray(data.files) ? data.files.join(', ') : '(manifest is malformed - run reinstall)'}`);
}

const HELP = `win-nice - installer for the win-nice Windows tools

usage: win-nice <command>

commands:
  install            install wnice.exe, one <tool>.exe per tool name and the
                     Git Bash shims into %LOCALAPPDATA%\\win-nice\\bin and add
                     that directory to the user PATH (what postinstall runs)
  uninstall          remove the installed files and the PATH entry
  reinstall          uninstall, then install from this package version
  status             show the installed version, directory and files
  skill install      add the win-nice reference skill for Claude Code / Codex
  skill uninstall    remove that skill
  help               this help

environment:
  WIN_NICE_HOME        install root instead of %LOCALAPPDATA%\\win-nice
  WIN_NICE_SKILL_HOME  home directory the skill paths are resolved against
  WIN_NICE_NO_PATH     set to anything: never touch the user PATH

examples:
  npm install -g win-nice      install (postinstall runs 'win-nice install')
  npx win-nice status
  npx win-nice uninstall       'npm uninstall -g' alone does not remove the tools
  npx win-nice skill install

The tools themselves document their options: run 'wnice --help' or
'<tool> --help' (idle, capc, capt, capm, caps, capn, admin, uiup, ...).`;

function main() {
  const cmd = process.argv[2] || 'install';
  const updatePath = !process.env.WIN_NICE_NO_PATH;
  switch (cmd) {
    case 'help':
    case '-h':
    case '--help':
      console.log(HELP);
      break;
    case 'install':
      install({ updatePath });
      break;
    case 'uninstall':
      uninstall({ updatePath });
      break;
    case 'reinstall':
      uninstall({ updatePath });
      install({ updatePath });
      break;
    case 'status':
      status();
      break;
    case 'skill': {
      const sub = process.argv[3];
      if (sub === 'install') {
        const results = installSkill();
        if (results.some((r) => !r.installed)) process.exitCode = 1;
      } else if (sub === 'uninstall') {
        const results = uninstallSkill();
        // 'missing' (nothing to remove) isn't a failure; a stripped/foreign
        // marker (skip-due-to-conflict) is - it means removal didn't happen
        // when a real file was there.
        if (results.some((r) => !r.removed && r.reason !== 'missing')) process.exitCode = 1;
      } else {
        console.error(`unknown skill command: ${sub}`);
        console.error('usage: win-nice skill <install|uninstall>');
        process.exitCode = 1;
      }
      break;
    }
    default:
      console.error(`unknown command: ${cmd}`);
      console.error('usage: win-nice <install|uninstall|reinstall|status|skill|help>');
      console.error("Run 'win-nice help' for details.");
      process.exitCode = 1;
  }
}

main();
