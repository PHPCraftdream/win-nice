#!/usr/bin/env node
'use strict';
// Structure linter: keeps the tree small enough to read.
//   1. no code file longer than MAX_FILE_LINES lines;
//   2. no directory that directly contains code holds more than
//      MAX_DIR_ENTRIES entries (files and subdirectories together).
// Entries are tracked files plus untracked-but-not-ignored ones, so a new file
// is counted before it is committed. Directories with no code files of their
// own (docs, assets) are exempt.
const fs = require('fs');
const path = require('path');
const { execFileSync } = require('child_process');

const MAX_FILE_LINES = 1000;
const MAX_DIR_ENTRIES = 7;
const CODE_EXTENSIONS = new Set([
  '.js', '.mjs', '.cjs', '.ts', '.ps1', '.psm1', '.cs', '.rs', '.bat', '.cmd',
]);
const SKIP_DIRS = new Set(['.git', 'node_modules', 'target', 'worktrees']);

// Violations that predate the rules. A ratchet: the measured value may shrink,
// never grow. Delete an entry as soon as its path complies (the linter prints
// a note when one can be removed or tightened).
const BASELINE = {
  files: {},
  dirs: {},
};

function toPosix(p) {
  return p.split(path.sep).join('/');
}

function walk(root, rel = '') {
  const out = [];
  for (const entry of fs.readdirSync(path.join(root, rel), { withFileTypes: true })) {
    if (SKIP_DIRS.has(entry.name)) continue;
    const childRel = rel ? `${rel}/${entry.name}` : entry.name;
    if (entry.isDirectory()) out.push(...walk(root, childRel));
    else out.push(childRel);
  }
  return out;
}

function listFiles(root, useGit = true) {
  if (useGit) {
    try {
      const raw = execFileSync(
        'git', ['ls-files', '--cached', '--others', '--exclude-standard', '-z'],
        { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'], maxBuffer: 64 * 1024 * 1024 }
      );
      // A file deleted in the working tree is still "cached" until committed.
      return [...new Set(raw.split('\0').filter(Boolean))]
        .filter((f) => fs.existsSync(path.join(root, f)));
    } catch {
      // Not a git checkout (or git missing): walk the directory instead.
    }
  }
  return walk(root);
}

function countLines(file) {
  const text = fs.readFileSync(file, 'utf8');
  if (text === '') return 0;
  return text.split('\n').length - (text.endsWith('\n') ? 1 : 0);
}

function isCode(file) {
  return CODE_EXTENSIONS.has(path.extname(file).toLowerCase());
}

// Direct children (file and subdirectory names) of every directory.
function directoryEntries(files) {
  const dirs = new Map();
  for (const file of files) {
    const parts = file.split('/');
    for (let i = 0; i < parts.length; i++) {
      const dir = i === 0 ? '.' : parts.slice(0, i).join('/');
      if (!dirs.has(dir)) dirs.set(dir, new Set());
      dirs.get(dir).add(parts[i]);
    }
  }
  return dirs;
}

function lintTree(root, {
  files = listFiles(root),
  baseline = BASELINE,
  maxFileLines = MAX_FILE_LINES,
  maxDirEntries = MAX_DIR_ENTRIES,
} = {}) {
  const posixFiles = files.map(toPosix);
  const violations = [];
  const notes = [];

  for (const file of posixFiles.filter(isCode)) {
    const lines = countLines(path.join(root, file));
    const allowed = baseline.files[file];
    if (lines > maxFileLines && (allowed === undefined || lines > allowed)) {
      violations.push({
        kind: 'file-lines', path: file, actual: lines, limit: allowed === undefined ? maxFileLines : allowed,
        hint: allowed === undefined
          ? `split it into files of at most ${maxFileLines} lines`
          : `baseline allows ${allowed}; it may shrink but not grow`,
      });
    } else if (allowed !== undefined && lines < allowed) {
      notes.push(lines > maxFileLines
        ? `${file}: now ${lines} lines, lower its baseline from ${allowed}`
        : `${file}: complies (${lines} lines), remove its baseline entry`);
    }
  }

  const entries = directoryEntries(posixFiles);
  const codeDirs = new Set(posixFiles.filter(isCode).map((f) => (f.includes('/') ? f.slice(0, f.lastIndexOf('/')) : '.')));
  for (const dir of codeDirs) {
    const count = (entries.get(dir) || new Set()).size;
    const allowed = baseline.dirs[dir];
    if (count > maxDirEntries && (allowed === undefined || count > allowed)) {
      violations.push({
        kind: 'dir-entries', path: dir, actual: count, limit: allowed === undefined ? maxDirEntries : allowed,
        hint: allowed === undefined
          ? `group files into subdirectories so it holds at most ${maxDirEntries} entries`
          : `baseline allows ${allowed}; it may shrink but not grow`,
      });
    } else if (allowed !== undefined && count < allowed) {
      notes.push(count > maxDirEntries
        ? `${dir}/: now ${count} entries, lower its baseline from ${allowed}`
        : `${dir}/: complies (${count} entries), remove its baseline entry`);
    }
  }
  for (const dir of Object.keys(baseline.dirs)) {
    if (!codeDirs.has(dir)) notes.push(`${dir}/: no longer holds code, remove its baseline entry`);
  }
  for (const file of Object.keys(baseline.files)) {
    if (!posixFiles.includes(file)) notes.push(`${file}: no longer exists, remove its baseline entry`);
  }

  violations.sort((a, b) => a.path.localeCompare(b.path));
  return { violations, notes };
}

function main() {
  const root = path.join(__dirname, '..', '..');
  const { violations, notes } = lintTree(root);
  for (const note of notes) console.error(`lint-structure: note - ${note}`);
  if (violations.length === 0) {
    console.error('lint-structure: ok');
    return;
  }
  console.error(`lint-structure: FAIL - ${violations.length} violation(s):`);
  for (const v of violations) {
    const what = v.kind === 'file-lines' ? `${v.actual} lines (limit ${v.limit})` : `${v.actual} entries (limit ${v.limit})`;
    console.error(`  - ${v.path}${v.kind === 'dir-entries' ? '/' : ''}: ${what} - ${v.hint}`);
  }
  process.exitCode = 1;
}

if (require.main === module) main();

module.exports = {
  lintTree, listFiles, directoryEntries, BASELINE, MAX_FILE_LINES, MAX_DIR_ENTRIES, CODE_EXTENSIONS,
};
