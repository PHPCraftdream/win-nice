'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const crypto = require('node:crypto');

const {
  install,
  TOOL_NAMES,
  EXE_NAME,
  SIDECAR_NAME,
  expectedInstalledFiles,
  shimText,
} = require('../../install/install');
const { uninstall } = require('../../install/uninstall');
const paths = require('../../install/paths');
const manifest = require('../../install/manifest');

const { FAKE_EXE, freshHome, withHome } = require('./home');

// ---- install() as ONE transaction (Phase R + Phase C + manifest write) ----

// Distinct version bytes so a rollback can prove the ORIGINAL file came back.
const V1 = Buffer.from('wnice version ONE');
const V2 = Buffer.from('wnice version TWO --newer--');

// Installs a source exe with EXACTLY these bytes: overrides
// WIN_NICE_SOURCE_BIN with a fresh temp dir around fn (restoring the value
// withHome() set afterwards).
function withSourceBin(bytes, fn) {
  const prev = process.env.WIN_NICE_SOURCE_BIN;
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'win-nice-srcbin-'));
  fs.writeFileSync(path.join(dir, EXE_NAME), bytes);
  process.env.WIN_NICE_SOURCE_BIN = dir;
  try {
    return fn();
  } finally {
    if (prev === undefined) delete process.env.WIN_NICE_SOURCE_BIN;
    else process.env.WIN_NICE_SOURCE_BIN = prev;
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

// _fs seam whose writeFileSync throws EPERM on its Nth call (everything else
// passes through to the real fs).
function writeFailingFs(failOnCall) {
  let writes = 0;
  return new Proxy(fs, {
    get(target, prop) {
      if (prop === 'writeFileSync') {
        return (...args) => {
          writes++;
          if (writes === failOnCall) {
            throw Object.assign(new Error('EPERM: simulated write failure'), { code: 'EPERM' });
          }
          return target.writeFileSync(...args);
        };
      }
      return target[prop];
    },
  });
}

// After a rolled-back V1 -> V2 upgrade, EVERYTHING must be exactly the V1
// install: the 31 names, V1 exe bytes, V1 sidecar/shim texts, no .old-*
// leftovers, and a manifest still listing the 31 contract names.
function assertV1InstallIntact(dir) {
  const expected = expectedInstalledFiles();
  assert.deepEqual(fs.readdirSync(dir).sort(), expected.slice().sort(), 'exactly the 31 names, no .old-* leftovers');
  assert.ok(fs.readFileSync(path.join(dir, EXE_NAME)).equals(V1), 'wnice.exe must be back at V1 bytes');
  for (const t of TOOL_NAMES) {
    assert.ok(fs.readFileSync(path.join(dir, `${t}.exe`)).equals(V1), `${t}.exe must be back at V1 bytes`);
  }
  assert.equal(fs.readFileSync(path.join(dir, SIDECAR_NAME), 'utf8'), `${manifest.MARKER}\n`, 'sidecar text');
  for (const name of [...TOOL_NAMES, 'wnice']) {
    assert.equal(fs.readFileSync(path.join(dir, name), 'utf8'), shimText(name), `shim ${name} must be the V1 text`);
  }
  assert.deepEqual(manifest.read(paths.manifestPath()).files, expected, 'manifest must still list the 31 names');
}

test('a Phase R rename failure rolls back the renames already done', () => {
  withHome(freshHome(), () => {
    withSourceBin(V1, () => install({ updatePath: false }));
    const dir = paths.binDir();

    const originalRenameSync = fs.renameSync;
    let renames = 0;
    // Calls 1-2 (wnice.exe, idle.exe) succeed; call 3 (belownormal.exe) hits
    // the lock. Only call 3 fails - the rollback's rename-backs must succeed.
    fs.renameSync = function (target, ...args) {
      renames++;
      if (renames === 3) {
        throw Object.assign(new Error('EPERM: in use'), { code: 'EPERM' });
      }
      return originalRenameSync.call(this, target, ...args);
    };
    try {
      withSourceBin(V2, () => {
        assert.throws(() => install({ updatePath: false }), /close running win-nice tools/);
      });
    } finally {
      fs.renameSync = originalRenameSync;
    }

    assertV1InstallIntact(dir);
  });
});

test('a failure after the sidecar write restores the pre-upgrade sidecar and shims', () => {
  withHome(freshHome(), () => {
    withSourceBin(V1, () => install({ updatePath: false }));
    const dir = paths.binDir();

    // writeFileSync call 1 = sidecar (succeeds), call 2 = first shim (throws).
    const _fs = writeFailingFs(2);
    withSourceBin(V2, () => {
      assert.throws(() => install({ updatePath: false, _fs }), /close running win-nice tools/);
    });

    assertV1InstallIntact(dir);
  });
});

test('a failure after the second shim write restores every overwritten shim', () => {
  withHome(freshHome(), () => {
    withSourceBin(V1, () => install({ updatePath: false }));
    const dir = paths.binDir();

    // writeFileSync calls 1-3 (sidecar, shim idle, shim belownormal) succeed,
    // call 4 (shim abovenormal) throws.
    const _fs = writeFailingFs(4);
    withSourceBin(V2, () => {
      assert.throws(() => install({ updatePath: false, _fs }), /close running win-nice tools/);
    });

    assertV1InstallIntact(dir);
  });
});

test('a manifest write failure rolls back the whole upgrade', () => {
  withHome(freshHome(), () => {
    withSourceBin(V1, () => install({ updatePath: false }));
    const dir = paths.binDir();
    const manifestFile = paths.manifestPath();
    // The exact manifest bytes the V1 install left behind - the rollback's
    // restore write must reproduce them byte for byte.
    const v1ManifestBytes = fs.readFileSync(manifestFile);

    const originalWriteFileSync = fs.writeFileSync;
    let manifestWrites = 0;
    fs.writeFileSync = function (target, ...args) {
      if (String(target).endsWith('install-manifest.json')) {
        manifestWrites++;
        if (manifestWrites === 1) {
          // Simulate a PARTIAL write landing on disk before the failure, so
          // the file is corrupt at the moment install() throws.
          const data = args[0];
          originalWriteFileSync.call(this, target, Buffer.from(data).slice(0, Math.max(1, Math.floor(Buffer.byteLength(data) / 2))));
          throw Object.assign(new Error('EPERM: manifest write failed'), { code: 'EPERM' });
        }
        // Subsequent manifest writes (the rollback's own restore) pass through.
      }
      return originalWriteFileSync.call(this, target, ...args);
    };
    try {
      withSourceBin(V2, () => {
        assert.throws(() => install({ updatePath: false }), /close running win-nice tools/);
      });
    } finally {
      fs.writeFileSync = originalWriteFileSync;
    }

    assert.ok(manifestWrites >= 2, 'the corrupting write and the rollback restore must both have run');
    // The corrupt partial manifest must be gone: the rollback restored the
    // V1 manifest bytes EXACTLY (Buffer.equals, not just a parseable JSON).
    assert.ok(
      fs.readFileSync(manifestFile).equals(v1ManifestBytes),
      'manifest must be byte-identical to the V1 manifest after rollback'
    );
    assertV1InstallIntact(dir);
  });
});

test('rename-aside really happens: an upgrade succeeds while every unlink of an exe fails with EBUSY', () => {
  withHome(freshHome(), () => {
    withSourceBin(V1, () => install({ updatePath: false }));
    const dir = paths.binDir();

    const originalUnlinkSync = fs.unlinkSync;
    fs.unlinkSync = function (target, ...args) {
      if (/\.exe(\.old-\d+)?$/.test(String(target))) {
        throw Object.assign(new Error('EBUSY: in use'), { code: 'EBUSY' });
      }
      return originalUnlinkSync.call(this, target, ...args);
    };
    try {
      withSourceBin(V2, () => {
        // Must SUCCEED: Phase R renamed the old exes aside instead of
        // unlinking them (the old unlink-then-link code would have thrown
        // EBUSY here).
        install({ updatePath: false });
      });
    } finally {
      fs.unlinkSync = originalUnlinkSync;
    }

    assert.ok(fs.readFileSync(path.join(dir, EXE_NAME)).equals(V2), 'wnice.exe must have V2 bytes');
    for (const t of TOOL_NAMES) {
      assert.ok(fs.readFileSync(path.join(dir, `${t}.exe`)).equals(V2), `${t}.exe must have V2 bytes`);
    }
    assert.equal(fs.readFileSync(path.join(dir, SIDECAR_NAME), 'utf8'), `${manifest.MARKER}\n`);
    for (const name of [...TOOL_NAMES, 'wnice']) {
      assert.equal(fs.readFileSync(path.join(dir, name), 'utf8'), shimText(name), `shim ${name} text`);
    }

    // The renamed-aside V1 exes are locked (EBUSY) DURING the upgrade, so the
    // in-install sweep legitimately leaves them behind ("a still-running old
    // exe can only be removed later"). They must all be OUR strict pattern;
    // with the lock gone they are removable - and then the dir is exactly the
    // 31 contract files (no leftovers).
    const { isOldLeftoverName } = require('../../install/install');
    const leftovers = fs.readdirSync(dir).filter((name) => name.includes('.old-'));
    assert.ok(leftovers.length > 0, 'the upgrade must have used rename-aside, leaving .old-N files behind');
    for (const name of leftovers) {
      assert.equal(isOldLeftoverName(name), true, `${name} must match the strict leftover pattern`);
    }
    for (const name of leftovers) fs.unlinkSync(path.join(dir, name));
    assert.deepEqual(fs.readdirSync(dir).sort(), expectedInstalledFiles().slice().sort());
    assert.deepEqual(manifest.read(paths.manifestPath()).files, expectedInstalledFiles());
  });
});

// ---- snapshot read failures abort BEFORE anything is written ----

test('a snapshot read failure aborts the upgrade before anything changes', () => {
  withHome(freshHome(), () => {
    withSourceBin(V1, () => install({ updatePath: false }));
    const dir = paths.binDir();
    const manifestFile = paths.manifestPath();
    const v1ManifestBytes = fs.readFileSync(manifestFile);
    // V1 bytes of every one of the 31 contract names, captured BEFORE the
    // upgrade - the abort must leave all of them untouched.
    const v1Bytes = new Map(expectedInstalledFiles().map((name) => [name, fs.readFileSync(path.join(dir, name))]));

    const originalReadFileSync = fs.readFileSync;
    fs.readFileSync = function (target, ...args) {
      // EACCES ONLY for the sidecar SNAPSHOT read - fs.readFileSync(path)
      // with NO encoding arg. isOwnedArtifact's ownership checks read the
      // sidecar with 'utf8' earlier in install() and must keep working, so
      // the missing-encoding call (which only the prevBytes snapshot makes)
      // is the discriminator.
      if (String(target).endsWith(SIDECAR_NAME) && args.length === 0) {
        throw Object.assign(new Error('EACCES: permission denied'), { code: 'EACCES' });
      }
      return originalReadFileSync.call(this, target, ...args);
    };
    try {
      withSourceBin(V2, () => {
        assert.throws(() => install({ updatePath: false }), /could not back up/);
      });
    } finally {
      fs.readFileSync = originalReadFileSync;
    }

    // NOTHING changed: all 31 names still present with their V1 bytes...
    assert.deepEqual(fs.readdirSync(dir).sort(), expectedInstalledFiles().slice().sort(), 'no .old-* leftovers, nothing removed or added');
    for (const [name, bytes] of v1Bytes) {
      assert.ok(
        fs.readFileSync(path.join(dir, name)).equals(bytes),
        `${name} must still have its V1 bytes`
      );
    }
    // ...and the manifest is still the V1 manifest.
    assert.ok(fs.readFileSync(manifestFile).equals(v1ManifestBytes), 'manifest unchanged');
  });
});

test('a previous-manifest snapshot failure aborts the upgrade before anything changes', () => {
  withHome(freshHome(), () => {
    withSourceBin(V1, () => install({ updatePath: false }));
    const dir = paths.binDir();
    const manifestFile = paths.manifestPath();
    const v1ManifestBytes = fs.readFileSync(manifestFile);
    const v1Bytes = new Map(expectedInstalledFiles().map((name) => [name, fs.readFileSync(path.join(dir, name))]));

    const originalReadFileSync = fs.readFileSync;
    let snapshotReads = 0;
    fs.readFileSync = function (target, ...args) {
      // Only the snapshot reads the manifest as raw bytes (no encoding arg).
      if (String(target) === manifestFile && args.length === 0) {
        snapshotReads++;
        throw Object.assign(new Error('EACCES: permission denied'), { code: 'EACCES' });
      }
      return originalReadFileSync.call(this, target, ...args);
    };
    try {
      withSourceBin(V2, () => {
        assert.throws(() => install({ updatePath: false }), /could not back up/);
      });
    } finally {
      fs.readFileSync = originalReadFileSync;
    }

    assert.equal(snapshotReads, 1, 'the manifest snapshot read must have been the injected failure');
    assert.deepEqual(fs.readdirSync(dir).sort(), expectedInstalledFiles().slice().sort(), 'no .old-* leftovers, every exe name still active');
    for (const [name, bytes] of v1Bytes) {
      assert.ok(fs.readFileSync(path.join(dir, name)).equals(bytes), `${name} must still have its V1 bytes`);
    }
    assert.ok(fs.readFileSync(manifestFile).equals(v1ManifestBytes), 'manifest unchanged');
  });
});

test('a write that partially writes a NEW shim then throws leaves no stray file', () => {
  withHome(freshHome(), () => {
    const dir = paths.binDir();

    // First-install scenario: fresh home, nothing existed before. Write 1 is
    // the sidecar; write 2 (the first NEW shim) lands HALF its bytes on disk
    // and THEN throws - a destination registered via track() before the write
    // must still be deleted by the rollback, leaving no stray file behind.
    let writes = 0;
    let failedFile = null;
    const _fs = new Proxy(fs, {
      get(target, prop) {
        if (prop === 'writeFileSync') {
          return (file, data, ...args) => {
            writes++;
            if (writes === 2) {
              failedFile = String(file);
              const half = data.slice(0, Math.floor(data.length / 2));
              target.writeFileSync(file, half, ...args);
              throw Object.assign(new Error('EPERM: simulated write failure'), { code: 'EPERM' });
            }
            return target.writeFileSync(file, data, ...args);
          };
        }
        return target[prop];
      },
    });

    assert.throws(
      () => install({ updatePath: false, _fs }),
      /close running win-nice tools/
    );

    assert.equal(path.basename(failedFile), TOOL_NAMES[0], 'the injected failure must hit the first shim');
    // The dir itself may exist (mkdir ran), but NONE of the 31 contract names
    // may have survived - no stray half-written shim/sidecar.
    const entries = fs.existsSync(dir) ? fs.readdirSync(dir) : [];
    const contract = new Set(expectedInstalledFiles());
    for (const entry of entries) {
      assert.equal(contract.has(entry), false, `stray file survived rollback: ${entry}`);
    }
    assert.equal(entries.length, 0, 'rollback must have deleted every file the failed install created');
  });
});

test('manifest-less uninstall renames a locked owned exe aside', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    const dir = paths.binDir();
    // Simulate the manifest-less recovery path.
    fs.unlinkSync(paths.manifestPath());

    const originalUnlinkSync = fs.unlinkSync;
    // EBUSY ONLY for plain .exe names (NOT for .old-N leftovers, so the final
    // sweep can delete the leftover this rename creates).
    fs.unlinkSync = function (target, ...args) {
      if (/\.exe$/.test(String(target))) {
        throw Object.assign(new Error('EBUSY: in use'), { code: 'EBUSY' });
      }
      return originalUnlinkSync.call(this, target, ...args);
    };
    let results;
    try {
      results = uninstall({ updatePath: false });
    } finally {
      fs.unlinkSync = originalUnlinkSync;
    }

    // The locked wnice.exe must have been renamed aside (and reported as such)...
    const wniceResult = results.find((r) => r.file === path.join(dir, EXE_NAME));
    assert.ok(wniceResult, 'wnice.exe must appear in the results');
    assert.match(wniceResult.reason, /in use - renamed aside to wnice\.exe\.old-\d+/);
    // ...and must no longer exist under its active name afterwards.
    assert.equal(fs.existsSync(path.join(dir, EXE_NAME)), false, 'locked wnice.exe must be gone from its active name');

    // Cleanup so later tests / withHome() see a sane state.
    fs.rmSync(dir, { recursive: true, force: true });
  });
});
