'use strict';
const fs = require('fs');
const path = require('path');

const MARKER = 'win-nice: managed-file';

function hasMarker(filePath) {
  try {
    return fs.readFileSync(filePath, 'utf8').includes(MARKER);
  } catch {
    return false;
  }
}

function write(manifestFile, data) {
  fs.mkdirSync(path.dirname(manifestFile), { recursive: true });
  fs.writeFileSync(manifestFile, JSON.stringify(data, null, 2) + '\n');
}

function read(manifestFile) {
  if (!fs.existsSync(manifestFile)) return null;
  try {
    return JSON.parse(fs.readFileSync(manifestFile, 'utf8'));
  } catch {
    return null;
  }
}

module.exports = { MARKER, hasMarker, write, read };
