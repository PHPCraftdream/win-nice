#!/usr/bin/env node
'use strict';
const { install } = require('./install');
const { uninstall } = require('./uninstall');
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
  console.log(`files: ${data.files.join(', ')}`);
}

function main() {
  const cmd = process.argv[2] || 'install';
  const updatePath = !process.env.WIN_NICE_NO_PATH;
  switch (cmd) {
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
    default:
      console.error(`unknown command: ${cmd}`);
      console.error('usage: win-nice <install|uninstall|reinstall|status>');
      process.exitCode = 1;
  }
}

main();
