'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const { lintTree, listFiles, MAX_FILE_LINES, MAX_DIR_ENTRIES } = require('./lint-structure');

const none = { files: {}, dirs: {} };

// Builds a throwaway tree: { 'src/a.js': 3, 'docs/x.md': 1 } = file -> line count.
function withTree(spec, fn) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'win-nice-lint-'));
  try {
    for (const [rel, lines] of Object.entries(spec)) {
      const file = path.join(root, rel);
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.writeFileSync(file, 'x\n'.repeat(lines));
    }
    fn(root);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
}

const lint = (root, baseline = none) => lintTree(root, { files: listFiles(root, false), baseline });

test('real repo complies (modulo the documented baseline)', () => {
  const { violations } = lintTree(path.join(__dirname, '..', '..'));
  assert.deepEqual(violations, []);
});

test('a code file of exactly the limit passes, one line more fails', () => {
  withTree({ 'src/ok.rs': MAX_FILE_LINES }, (root) => assert.deepEqual(lint(root).violations, []));
  withTree({ 'src/big.rs': MAX_FILE_LINES + 1 }, (root) => {
    const { violations } = lint(root);
    assert.equal(violations.length, 1);
    assert.equal(violations[0].kind, 'file-lines');
    assert.equal(violations[0].path, 'src/big.rs');
    assert.equal(violations[0].actual, MAX_FILE_LINES + 1);
  });
});

test('a long non-code file is not checked', () => {
  withTree({ 'docs/long.md': MAX_FILE_LINES * 3 }, (root) => assert.deepEqual(lint(root).violations, []));
});

test('a code directory of exactly the limit passes, one entry more fails', () => {
  const spec = (n) => Object.fromEntries(Array.from({ length: n }, (_, i) => [`src/f${i}.js`, 1]));
  withTree(spec(MAX_DIR_ENTRIES), (root) => assert.deepEqual(lint(root).violations, []));
  withTree(spec(MAX_DIR_ENTRIES + 1), (root) => {
    const { violations } = lint(root);
    assert.equal(violations.length, 1);
    assert.equal(violations[0].kind, 'dir-entries');
    assert.equal(violations[0].path, 'src');
    assert.equal(violations[0].actual, MAX_DIR_ENTRIES + 1);
  });
});

test('subdirectories count as one entry each, mixed with files', () => {
  const spec = {};
  for (let i = 0; i < 4; i++) spec[`src/f${i}.js`] = 1;
  for (let i = 0; i < 4; i++) spec[`src/sub${i}/deep.js`] = 1;
  withTree(spec, (root) => {
    const { violations } = lint(root);
    assert.deepEqual(violations.map((v) => [v.path, v.actual]), [['src', 8]]);
  });
});

test('a directory without code of its own is exempt, however crowded', () => {
  const spec = Object.fromEntries(Array.from({ length: 30 }, (_, i) => [`docs/r${i}.md`, 1]));
  withTree({ ...spec, 'src/a.js': 1 }, (root) => assert.deepEqual(lint(root).violations, []));
});

test('baseline tolerates the recorded size, rejects growth, and notes shrinkage', () => {
  const baseline = { files: { 'src/big.js': MAX_FILE_LINES + 5 }, dirs: {} };
  withTree({ 'src/big.js': MAX_FILE_LINES + 5 }, (root) => assert.deepEqual(lint(root, baseline).violations, []));
  withTree({ 'src/big.js': MAX_FILE_LINES + 6 }, (root) => {
    assert.equal(lint(root, baseline).violations.length, 1);
  });
  withTree({ 'src/big.js': MAX_FILE_LINES + 1 }, (root) => {
    const { violations, notes } = lint(root, baseline);
    assert.deepEqual(violations, []);
    assert.match(notes.join('\n'), /lower its baseline/);
  });
  withTree({ 'src/big.js': 10 }, (root) => {
    assert.match(lint(root, baseline).notes.join('\n'), /remove its baseline entry/);
  });
});

test('directory baseline tolerates the recorded count and rejects growth', () => {
  const spec = (n) => Object.fromEntries(Array.from({ length: n }, (_, i) => [`bin/f${i}.js`, 1]));
  const baseline = { files: {}, dirs: { bin: 10 } };
  withTree(spec(10), (root) => assert.deepEqual(lint(root, baseline).violations, []));
  withTree(spec(11), (root) => assert.equal(lint(root, baseline).violations.length, 1));
});

test('stale baseline entries for vanished paths produce notes, not failures', () => {
  const baseline = { files: { 'gone.js': 2000 }, dirs: { gone: 20 } };
  withTree({ 'src/a.js': 1 }, (root) => {
    const { violations, notes } = lint(root, baseline);
    assert.deepEqual(violations, []);
    assert.equal(notes.length, 2);
  });
});
