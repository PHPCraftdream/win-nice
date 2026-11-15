#!/usr/bin/env node
'use strict';
// Prints the CHANGELOG.md section for a version to stdout, ready for
// `gh release create --notes-file`. Accepts "v0.4.0" or "0.4.0"; anything
// else is a usage error. Used by release.yml's "Release notes from
// CHANGELOG.md" step.
const fs = require('fs');
const path = require('path');

const repoRoot = path.join(__dirname, '..');

// Same heading shape release-check.js validates
// (changelogVersionHeadingPattern): "## [X.Y.Z] - YYYY-MM-DD".
const versionHeadingPattern = /^## \[(\d+\.\d+\.\d+)\] - (\d{4})-(\d{2})-(\d{2})$/;
// Markdown link-reference definition (e.g. "[0.3.0]: https://...compare/...").
const linkReferencePattern = /^\[.+\]:\s/;

function fail(message) {
  console.error(`release-notes: ${message}`);
  process.exit(1);
}

function usage() {
  fail('usage: node scripts/release-notes.js <tag-or-version> (e.g. v0.4.0 or 0.4.0)');
}

// Strip one leading 'v'; anything not matching X.Y.Z is a usage error.
function requestedVersion(argv) {
  if (argv.length !== 1) return null;
  const version = argv[0].replace(/^v/, '');
  return /^\d+\.\d+\.\d+$/.test(version) ? version : null;
}

// Lines between the version's heading and the next '## ' heading (exclusive),
// or null when the CHANGELOG has no heading for the version.
function sectionBodyLines(changelog, version) {
  const lines = String(changelog).split(/\r\n?|\n/);
  const start = lines.findIndex((line) => {
    const match = versionHeadingPattern.exec(line);
    return match !== null && match[1] === version;
  });
  if (start === -1) return null;
  const body = [];
  for (let i = start + 1; i < lines.length && !lines[i].startsWith('## '); i++) {
    body.push(lines[i]);
  }
  return body;
}

// Trailing markdown link-reference definitions (the compare-link block at
// the file bottom only belongs to the last section; stripping defensively is
// correct for any section) and blank padding have no business in release
// notes.
function trimSection(body) {
  let end = body.length;
  while (end > 0) {
    const line = body[end - 1];
    if (line.trim() === '' || linkReferencePattern.test(line)) {
      end -= 1;
    } else {
      break;
    }
  }
  let begin = 0;
  while (begin < end && body[begin].trim() === '') begin += 1;
  return body.slice(begin, end);
}

const version = requestedVersion(process.argv.slice(2));
if (!version) usage();

const changelog = fs.readFileSync(path.join(repoRoot, 'CHANGELOG.md'), 'utf8');
const section = sectionBodyLines(changelog, version);
const body = trimSection(section === null ? [] : section);
if (body.length === 0) {
  fail(`no CHANGELOG.md section found for version ${version} (expected a "## [<version>] - YYYY-MM-DD" heading with body text)`);
}
console.log(body.join('\n'));
