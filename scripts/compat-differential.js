#!/usr/bin/env node
'use strict';
// Backward-compatibility differential gate: runs one fixed set of commands
// against the PUBLISHED wnice.exe of the reference version and against the
// freshly built wnice.exe, then fails on any difference in stdout, stderr or
// exit code. The reference is a real published release pinned in
// tests/compat/reference-version.txt - never "latest", so the comparison
// stays reproducible after a new version ships.
//
// Exit codes: 0 = no unexplained difference and no stale allowlist entry;
// 1 = a difference, a stale/unusable allowlist entry, or a case that could
// not run in either side; 2 = bad usage or unreadable/inconsistent input.
//
// CI (the workflow downloads the tarball; this script never fetches anything):
//   node --require ./scripts/lib/hide-windows.js scripts/compat-differential.js \
//     --built bin/wnice.exe --tarball win-nice-0.4.0.tgz
// Local plumbing check with a local exe instead of a tarball - this is NOT
// valid for CI, which must always pass --tarball and never --reference,
// because --reference can point at any build and would silently weaken the
// gate:
//   node --require ./scripts/lib/hide-windows.js scripts/compat-differential.js \
//     --built bin/wnice.exe --reference C:\old\wnice.exe
//
// An intentional difference must be listed in
// tests/compat/differential-allowlist.json as
// { "case": <name>, "field": "stdout"|"stderr"|"exit", "reason": "..." }.
// An entry that stops differing fails the run, so it cannot rot.
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const zlib = require('node:zlib');
const { spawnSync } = require('node:child_process');

const repoRoot = path.join(__dirname, '..');
const compatDir = path.join(repoRoot, 'tests', 'compat');
const FIELDS = ['stdout', 'stderr', 'exit'];
// Field names an `expect` block may use; see checkExpect.
const EXPECT_KEYS = ['exit', 'stdoutIncludes', 'stderrIncludes'];
const CASE_TIMEOUT_MS = 120000;
const VERSION_PATTERN = /^\d+\.\d+\.\d+$/;

const USAGE = [
  'usage: node scripts/compat-differential.js --built <wnice.exe>',
  '         (--tarball <win-nice-x.y.z.tgz> | --reference <wnice.exe>)',
  '         [--work <dir>] [--cases <file>] [--allowlist <file>] [--verbose]',
].join('\n');

// exit 2 = the caller or the inputs are wrong, so nothing was compared.
function fatal(message) {
  console.error(`compat-differential: ${message}`);
  process.exit(2);
}

function badUsage(message) {
  console.error(`compat-differential: ${message}`);
  console.error(`compat-differential: ${USAGE}`);
  process.exit(2);
}

// --flag <value> pairs, in one table so an unknown flag cannot fall through.
const VALUE_FLAGS = new Map([
  ['--built', 'built'],
  ['--tarball', 'tarball'],
  ['--reference', 'reference'],
  ['--work', 'work'],
  ['--cases', 'cases'],
  ['--allowlist', 'allowlist'],
]);

function parseArgs(argv) {
  const opts = {
    built: null,
    tarball: null,
    reference: null,
    work: null,
    cases: path.join(compatDir, 'differential-cases.json'),
    allowlist: path.join(compatDir, 'differential-allowlist.json'),
    verbose: false,
  };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--verbose') {
      opts.verbose = true;
      continue;
    }
    const key = VALUE_FLAGS.get(arg);
    if (!key) badUsage(`unknown argument ${JSON.stringify(arg)}`);
    const value = argv[i + 1];
    // A following flag means the value was forgotten, not that it is a path.
    if (value === undefined || value.startsWith('--')) badUsage(`missing value for ${arg}`);
    opts[key] = value;
    i += 1;
  }
  if (!opts.built) badUsage('--built is required');
  if (opts.tarball && opts.reference) badUsage('pass exactly one of --tarball or --reference');
  if (!opts.tarball && !opts.reference) badUsage('one of --tarball or --reference is required');
  return opts;
}

function loadJson(file, label) {
  let text;
  try {
    text = fs.readFileSync(file, 'utf8');
  } catch (err) {
    fatal(`cannot read ${label} ${file}: ${err.message}`);
  }
  try {
    return JSON.parse(text);
  } catch (err) {
    fatal(`malformed JSON in ${label} ${file}: ${err.message}`);
  }
}

const isObject = (v) => v !== null && typeof v === 'object' && !Array.isArray(v);

function fileSize(file, label) {
  try {
    const stat = fs.statSync(file);
    if (stat.isFile()) return stat.size;
  } catch (err) {
    fatal(`cannot read ${label} ${file}: ${err.message}`);
  }
  fatal(`${label} ${file} is not a file`);
}

// npm tarballs are ustar: 512-byte headers with the name split over an
// optional prefix field, then the payload padded up to the next 512 block.
// Keeping this in-process avoids depending on an external `tar` binary.
const cstr = (buf) => buf.toString('utf8').replace(/\0.*$/s, '');

function readTarball(tgzPath) {
  let tar;
  try {
    tar = zlib.gunzipSync(fs.readFileSync(tgzPath));
  } catch (err) {
    fatal(`cannot read tarball ${tgzPath}: ${err.message}`);
  }
  const files = new Map();
  let off = 0;
  while (off + 512 <= tar.length) {
    const header = tar.subarray(off, off + 512);
    const name = cstr(header.subarray(0, 100));
    if (name === '') break; // two zero blocks end the archive
    const size = parseInt(cstr(header.subarray(124, 136)).trim(), 8) || 0;
    const prefix = cstr(header.subarray(345, 500));
    const full = prefix ? `${prefix}/${name}` : name;
    files.set(full, tar.subarray(off + 512, off + 512 + size));
    off += 512 + Math.ceil(size / 512) * 512; // entries are 512-byte aligned
  }
  return files;
}

function readPinnedVersion() {
  const file = path.join(compatDir, 'reference-version.txt');
  let text;
  try {
    text = fs.readFileSync(file, 'utf8');
  } catch (err) {
    fatal(`cannot read reference version file ${file}: ${err.message}`);
  }
  const version = text.trim();
  // exit 2 names the file: a typo here silently re-pins the whole gate.
  if (!VERSION_PATTERN.test(version)) fatal(`${file} must contain X.Y.Z (found "${version}")`);
  return version;
}

function loadCases(file) {
  const doc = loadJson(file, 'cases file');
  if (!isObject(doc) || !Array.isArray(doc.cases) || doc.cases.length === 0) {
    fatal(`cases file ${file} must be an object with a non-empty "cases" array`);
  }
  const where = `cases file ${path.basename(file)}`;
  const seen = new Set();
  for (const item of doc.cases) {
    // A usable name and tool are both load-bearing: the tool picks the exe
    // stem and the name keys the allowlist.
    const named = isObject(item) && ['name', 'tool'].every(
      (key) => typeof item[key] === 'string' && item[key] !== ''
    );
    const argsOk = named && Array.isArray(item.args)
      && item.args.every((a) => typeof a === 'string');
    const envOk = named && (item.env === undefined
      || (isObject(item.env) && Object.values(item.env).every((v) => typeof v === 'string')));
    const expected = item.expect;
    const expectOk = named && (expected === undefined
      || (isObject(expected)
        && Object.keys(expected).every((key) => EXPECT_KEYS.includes(key))
        && (expected.exit === undefined || Number.isInteger(expected.exit))
        && [expected.stdoutIncludes, expected.stderrIncludes].every(
          (value) => value === undefined || (typeof value === 'string' && value !== '')
        )));
    if (!named || !argsOk || !envOk || !expectOk) {
      fatal(`${where}: every case needs a name, tool, string args, an optional string-to-string env and an optional expect {exit, stdoutIncludes, stderrIncludes}`);
    }
    if (seen.has(item.name)) fatal(`${where}: duplicate case name ${item.name}`);
    seen.add(item.name);
  }
  return doc.cases;
}

// The allowlist is keyed by (case, field); anything malformed is exit 2 so a
// typo cannot silently turn into a "no entry" and hide a real difference.
function loadAllowlist(file, cases) {
  const doc = loadJson(file, 'allowlist');
  if (!isObject(doc) || !Array.isArray(doc.entries)) {
    fatal(`allowlist ${file} must be an object with an "entries" array`);
  }
  const known = new Set(cases.map((c) => c.name));
  const entries = new Map();
  doc.entries.forEach((entry, index) => {
    const where = `allowlist ${path.basename(file)} entry ${index}`;
    if (!isObject(entry)) fatal(`${where} must be an object`);
    const { case: name, field, reason } = entry;
    if (typeof name !== 'string' || !known.has(name)) {
      fatal(`${where}: unknown case ${JSON.stringify(name)}`);
    }
    if (!FIELDS.includes(field)) fatal(`${where}: field must be one of ${FIELDS.join(', ')}`);
    if (typeof reason !== 'string' || reason.trim() === '') {
      fatal(`${where}: reason must be a non-empty string`);
    }
    const key = allowKey(name, field);
    if (entries.has(key)) fatal(`${where}: duplicate entry for ${name} ${field}`);
    entries.set(key, { reason, matched: false });
  });
  return entries;
}

const allowKey = (name, field) => `${name}\u0000${field}`;

// --tarball is the CI path: the exe and package.json both come out of the
// published artifact, and the version inside it must be the pinned one.
function stageFromTarball(tarball, pinned, work) {
  const files = readTarball(tarball);
  const pkgBuf = files.get('package/package.json');
  if (!pkgBuf) fatal(`tarball ${tarball} has no package/package.json entry`);
  let pkg;
  try {
    pkg = JSON.parse(cstr(pkgBuf));
  } catch (err) {
    fatal(`tarball ${tarball} has malformed package/package.json: ${err.message}`);
  }
  if (pkg.version !== pinned) fatal(`version mismatch: tarball ${pkg.version}, pinned ${pinned}`);
  const exe = files.get('package/bin/wnice.exe');
  if (!exe) fatal(`tarball ${tarball} has no package/bin/wnice.exe entry`);
  const dest = path.join(work, 'reference', 'wnice.exe');
  fs.writeFileSync(dest, exe);
  return dest;
}

function stageFromReference(reference, work) {
  fileSize(reference, 'reference exe');
  const dest = path.join(work, 'reference', 'wnice.exe');
  fs.copyFileSync(reference, dest);
  return dest;
}

// The tools dispatch on argv[0]'s file stem, so each tool name needs its own
// copy of the same binary.
function stageExes(sourceExe, dir, tools) {
  fs.mkdirSync(dir, { recursive: true });
  for (const tool of tools) {
    const dest = path.join(dir, `${tool}.exe`);
    if (path.resolve(dest) === path.resolve(sourceExe)) continue;
    fs.copyFileSync(sourceExe, dest);
  }
}

function resolveCmd() {
  // The fallback keeps stripped environments usable; the existence check below
  // still fails loudly if it is wrong instead of running a different cmd.exe.
  const systemRoot = process.env.SystemRoot || 'C:\\Windows';
  const cmd = path.join(systemRoot, 'System32', 'cmd.exe');
  if (!fs.existsSync(cmd)) fatal(`cmd.exe not found at ${cmd} (SystemRoot=${systemRoot})`);
  return cmd;
}

// ${CMD} and ${WORKDIR} are the only placeholders: cases must not depend on
// the caller's PATH order or working directory.
function expand(text, vars) {
  return text
    .replace(/\$\{CMD\}/g, vars.cmd)
    .replace(/\$\{WORKDIR\}/g, vars.workdir);
}

function expandArgs(args, vars) {
  return args.map((arg) => expand(arg, vars));
}

// A case may override single environment variables (PATH, for the fake agent
// CLIs the cy/cx cases need); everything else is inherited, so both sides run
// in the same environment.
function caseEnv(item, vars) {
  const env = { ...process.env };
  for (const [key, value] of Object.entries(item.env ?? {})) {
    env[key] = expand(value, vars);
  }
  return env;
}

function runCase(exe, args, cwd, env) {
  const r = spawnSync(exe, args, {
    cwd,
    env,
    windowsHide: true,
    timeout: CASE_TIMEOUT_MS,
    encoding: 'buffer',
    stdin: 'ignore',
  });
  return {
    exit: typeof r.status === 'number' ? r.status : null,
    stdout: Buffer.isBuffer(r.stdout) ? r.stdout : Buffer.alloc(0),
    stderr: Buffer.isBuffer(r.stderr) ? r.stderr : Buffer.alloc(0),
    error: r.error ? String(r.error.message) : null,
  };
}

const differsIn = (field, a, b) => (field === 'exit' ? a !== b : !a.equals(b));

function render(field, value) {
  if (field === 'exit') return JSON.stringify(value);
  // JSON.stringify keeps trailing whitespace and CRLF differences visible;
  // the cap only stops a runaway output from flooding the log.
  return JSON.stringify(value.toString('utf8').slice(0, 4000));
}

// Some cases must prove WHICH path they took, not only that the two builds
// agree: an agent case whose stand-in was not found matches on both sides and
// would prove nothing. `expect` is therefore validated against BOTH builds.
function checkExpect(item, side, result) {
  const expect = item.expect ?? {};
  const problems = [];
  if (expect.exit !== undefined && result.exit !== expect.exit) {
    problems.push(`${side}: exit ${result.exit} != expected ${expect.exit}`);
  }
  for (const key of ['stdoutIncludes', 'stderrIncludes']) {
    const needle = expect[key];
    if (needle === undefined) continue;
    const field = key === 'stdoutIncludes' ? 'stdout' : 'stderr';
    if (!result[field].toString('utf8').includes(needle)) {
      problems.push(`${side}: ${field} does not contain ${JSON.stringify(needle)}`);
    }
  }
  return problems;
}

function main() {
  if (process.platform !== 'win32') {
    fatal(`this gate needs Windows (platform is ${process.platform}); wnice.exe is native`);
  }
  const opts = parseArgs(process.argv.slice(2));
  const pinned = readPinnedVersion();
  const cases = loadCases(opts.cases);
  const allowlist = loadAllowlist(opts.allowlist, cases);

  // Temp dirs are left behind on purpose: the exes and outputs are what a
  // failing CI run needs to reproduce a difference by hand.
  const work = opts.work
    ? (fs.mkdirSync(opts.work, { recursive: true }), path.resolve(opts.work))
    : fs.mkdtempSync(path.join(os.tmpdir(), 'wnice-compat-'));
  fs.mkdirSync(path.join(work, 'reference'), { recursive: true });

  // A wrong --built is the common CI mistake, so it is named before any
  // tarball work happens.
  const builtExe = path.resolve(opts.built);
  const builtSize = fileSize(builtExe, 'built exe');
  const referenceExe = opts.tarball
    ? stageFromTarball(opts.tarball, pinned, work)
    : stageFromReference(opts.reference, work);
  const referenceSize = fs.statSync(referenceExe).size;

  const cmd = resolveCmd();
  const shared = path.join(work, 'shared');
  fs.mkdirSync(shared, { recursive: true });
  fs.writeFileSync(path.join(shared, 'noop.bat'), '@echo off\r\nexit /b 0\r\n');

  // Fake agent CLIs for the cy/cx `--help` cases: .bat targets on a controlled
  // PATH that echo their arguments and exit with a fixed code, so the case
  // proves the flag reached the agent verbatim without a real agent installed.
  const agents = path.join(shared, 'agents');
  fs.mkdirSync(agents, { recursive: true });
  fs.writeFileSync(path.join(agents, 'claude.bat'), '@echo off\r\necho %*\r\nexit /b 7\r\n');
  fs.writeFileSync(path.join(agents, 'codex.bat'), '@echo off\r\necho %*\r\nexit /b 8\r\n');

  const tools = [...new Set(cases.map((c) => c.tool))].sort();
  stageExes(referenceExe, path.join(work, 'reference'), tools);
  stageExes(builtExe, path.join(work, 'built'), tools);

  console.error(
    `compat-differential: reference win-nice ${pinned} (${referenceSize} bytes), ` +
    `built ${builtExe} (${builtSize} bytes), ${cases.length} cases`
  );

  const counters = { differences: 0, allowlisted: 0, expectations: 0, errors: 0 };
  const vars = { cmd, workdir: shared };

  for (const item of cases) {
    const args = expandArgs(item.args, vars);
    const env = caseEnv(item, vars);
    const ref = runCase(path.join(work, 'reference', `${item.tool}.exe`), args, shared, env);
    const built = runCase(path.join(work, 'built', `${item.tool}.exe`), args, shared, env);

    // A timeout or spawn failure means the case never ran: comparing its
    // output would only report a byproduct of the failure itself.
    const broken = [
      ['reference', ref.error],
      ['built', built.error],
    ].filter(([, error]) => error);
    if (broken.length > 0) {
      counters.errors += 1;
      for (const [side, error] of broken) console.error(`ERROR ${item.name} ${side}: ${error}`);
      console.error(`  args: ${JSON.stringify(args)}`);
      continue;
    }

    const unmet = [
      ...checkExpect(item, 'reference', ref),
      ...checkExpect(item, 'built', built),
    ];
    if (unmet.length > 0) {
      counters.expectations += 1;
      for (const problem of unmet) console.error(`EXPECT ${item.name} ${problem}`);
      console.error(`  args:      ${JSON.stringify(args)}`);
      console.error(`  reference: exit=${ref.exit} stdout=${render('stdout', ref.stdout)} stderr=${render('stderr', ref.stderr)}`);
      console.error(`  built:     exit=${built.exit} stdout=${render('stdout', built.stdout)} stderr=${render('stderr', built.stderr)}`);
    }

    let perCase = 0;
    for (const field of FIELDS) {
      if (!differsIn(field, ref[field], built[field])) continue;
      perCase += 1;
      const entry = allowlist.get(allowKey(item.name, field));
      if (entry) {
        entry.matched = true;
        counters.allowlisted += 1;
        console.error(`allowlisted ${item.name} ${field}: ${entry.reason}`);
        continue;
      }
      counters.differences += 1;
      console.error(`DIFF ${item.name} ${field}`);
      console.error(`  reference: ${render(field, ref[field])}`);
      console.error(`  built:     ${render(field, built[field])}`);
      console.error(`  args:      ${JSON.stringify(args)}`);
    }
    // Verbose also names identical cases, so silence means "not compared"
    // rather than "matched". A case with unmet expectations is reported above,
    // not matched.
    if (opts.verbose && perCase === 0 && unmet.length === 0) console.error(`match ${item.name}`);
  }

  const stale = [...allowlist.entries()].filter(([, entry]) => !entry.matched);
  for (const [key, entry] of stale) {
    const [name, field] = key.split('\u0000');
    const why = `stale allowlist entry ${name} ${field} (${entry.reason})`;
    console.error(`${why}: the values now match — remove it`);
  }

  const failed = counters.differences > 0
    || counters.expectations > 0
    || counters.errors > 0
    || stale.length > 0;
  console.log(
    `compat-differential: ${cases.length} cases, ${counters.differences} differences, ` +
    `${counters.allowlisted} allowlisted, ${counters.expectations} unmet expectations, ` +
    `${stale.length} stale, ${counters.errors} errors`
  );
  if (failed) {
    process.exitCode = 1;
    return;
  }
  console.log('compat-differential: ok');
}

main();
