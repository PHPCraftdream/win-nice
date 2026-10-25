'use strict';
// Preloaded with `node --require` by the npm scripts: makes every child process
// spawned by the test/release tooling default to windowsHide:true. Without it
// a console child started by a node process that has no console (agents, IDEs,
// CI wrappers) opens a visible window - hundreds of them during a test run.
const cp = require('child_process');

const isOptions = (v) => v !== null && typeof v === 'object' && !Array.isArray(v);

function withHidden(args) {
  const out = Array.from(args);
  const i = out.findIndex(isOptions);
  if (i >= 0) {
    out[i] = { windowsHide: true, ...out[i] };
    return out;
  }
  // No options object: add one before a trailing callback, else at the end.
  const cb = out.length > 0 && typeof out[out.length - 1] === 'function';
  out.splice(cb ? out.length - 1 : out.length, 0, { windowsHide: true });
  return out;
}

for (const name of ['spawn', 'spawnSync', 'execFile', 'execFileSync', 'exec', 'execSync']) {
  const original = cp[name];
  cp[name] = function patched(...args) {
    return original.apply(this, withHidden(args));
  };
}
