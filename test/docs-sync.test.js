'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

// Guards against README.md and skills/win-nice/SKILL.md re-diverging on the
// %-argument fail-closed behavior (SKILL.md is installed for AI agents to
// read - stale wording there could make an agent believe a command ran when
// it actually failed closed with exit 1).
//
// The check is scoped to each doc's argument-safety section, not the whole
// file: the phrase "fails closed" independently appears in README's Job
// Object breakaway discussion (### capc), so a whole-document substring
// search would still pass if only the argument-safety section reverted to
// stale wording.
const DOCS = [
  { file: 'README.md', heading: 'Argument handling' },
  { file: path.join('skills', 'win-nice', 'SKILL.md'), heading: 'Argument safety' },
];

// Inside that section, both docs must still state that any `%` argument
// makes the cmd.exe /c fallback refuse the whole command (exit 1, stderr
// message). The long phrase is matched after whitespace normalization, so
// the two docs may line-wrap it differently. If the wording changes on
// purpose, update REQUIRED_PHRASES and BOTH docs in the same commit.
const REQUIRED_PHRASES = [
  'fails closed',
  'if any argument contains `%`, the tool refuses to run, prints an error to ' +
    'stderr, and exits with code `1`',
];

const normalize = (s) => s.replace(/\s+/g, ' ');

// Text from the given heading to the next ATX heading that is not inside a
// ``` code fence (or to end of document).
function extractSection(md, heading, doc) {
  const escaped = heading.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = md.match(new RegExp(`^#{1,6} ${escaped}\\s*$`, 'm'));
  assert.ok(match, `${doc}: no heading "${heading}" found (section renamed?)`);
  const body = md.slice(match.index + match[0].length);
  let offset = 0;
  let inFence = false;
  for (const line of body.split('\n')) {
    if (/^ {0,3}```/.test(line)) inFence = !inFence;
    else if (!inFence && /^#{1,6} /.test(line)) break;
    offset += line.length + 1;
  }
  return body.slice(0, offset);
}

test('README.md and skills/win-nice/SKILL.md agree on the %-argument fail-closed behavior', () => {
  for (const { file, heading } of DOCS) {
    const md = fs.readFileSync(path.join(__dirname, '..', file), 'utf8');
    const section = normalize(extractSection(md, heading, file));
    for (const phrase of REQUIRED_PHRASES) {
      assert.ok(
        section.includes(normalize(phrase)),
        `${file}'s "${heading}" section is missing "${phrase}" - if the wording ` +
          'changed intentionally, update REQUIRED_PHRASES and both docs together'
      );
    }
  }
});

// Exported so the extraction can be exercised against synthetic docs
// (drift-guard sanity checks) without editing the real files.
module.exports = { DOCS, REQUIRED_PHRASES, extractSection, normalize };
