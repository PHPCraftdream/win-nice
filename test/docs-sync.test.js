'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

// Guards against README.md and skills/win-nice/SKILL.md re-diverging on the
// %-argument fail-closed behavior (SKILL.md is installed for AI agents to
// read - stale wording there could make an agent believe a command ran when
// it actually failed closed with exit 1).
const SHARED_PHRASE = 'fails closed';

test('README.md and skills/win-nice/SKILL.md agree on the %-argument fail-closed behavior', () => {
  const readme = fs.readFileSync(path.join(__dirname, '..', 'README.md'), 'utf8');
  const skill = fs.readFileSync(path.join(__dirname, '..', 'skills', 'win-nice', 'SKILL.md'), 'utf8');

  assert.ok(readme.includes(SHARED_PHRASE), `README.md should contain "${SHARED_PHRASE}"`);
  assert.ok(skill.includes(SHARED_PHRASE), `SKILL.md should contain "${SHARED_PHRASE}"`);
});
