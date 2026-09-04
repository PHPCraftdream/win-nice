'use strict';
const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFileSync } = require('child_process');

// Guards mutating commands (install/uninstall/reinstall) against touching the
// real system PATH/install dir when run from inside a git clone - only a
// genuine package install (running from inside someone's node_modules) or an
// explicit WIN_NICE_HOME override proceeds.
function isSourceCheckout() {
  return fs.existsSync(path.join(__dirname, '..', '.git'));
}

// WIN_NICE_HOME overrides the install root - used by tests and by anyone who
// wants a non-default location. Real installs default to %LOCALAPPDATA%\win-nice.
function installRoot() {
  if (process.env.WIN_NICE_HOME) return process.env.WIN_NICE_HOME;
  const localAppData = process.env.LOCALAPPDATA || path.join(os.homedir(), 'AppData', 'Local');
  return path.join(localAppData, 'win-nice');
}

function binDir() {
  return path.join(installRoot(), 'bin');
}

function manifestPath() {
  return path.join(installRoot(), 'install-manifest.json');
}

function normalize(p) {
  return path.normalize(p).replace(/\\+$/, '').toLowerCase();
}

// Comparison-only stand-in for [Environment]::ExpandEnvironmentVariables: %VAR%
// references resolve case-insensitively against the current process env (Windows
// var names are case-insensitive and so is process.env lookup on win32); unknown
// or malformed references stay literal text instead of throwing or vanishing.
// Never used on anything we write back - the raw registry string is preserved
// exactly; this only lets add/remove recognize a raw %VAR% entry (e.g.
// %LOCALAPPDATA%\win-nice\bin) as the same location as its expanded form.
function expandEnvRefs(s) {
  return s.replace(/%([^%]*)%/g, (whole, name) => {
    const value = process.env[name];
    return value === undefined ? whole : value;
  });
}

// Two PATH entries point at the same location if their %VAR% references expand
// to the same directories, even though the registry stores the raw text.
function comparisonForm(p) {
  return normalize(expandEnvRefs(p));
}

function addToPathString(currentPath, dir) {
  const parts = currentPath.split(';').filter(Boolean);
  const target = comparisonForm(dir);
  const already = parts.some((p) => comparisonForm(p) === target);
  if (already) return currentPath;
  return [...parts, dir].join(';');
}

function removeFromPathString(currentPath, dir) {
  const parts = currentPath.split(';').filter(Boolean);
  const target = comparisonForm(dir);
  return parts.filter((p) => comparisonForm(p) !== target).join(';');
}

// Windows PowerShell is a fixed system dependency. Passing only its bare
// name to CreateProcess lets libuv search the current directory before PATH,
// so a powershell.exe planted next to `npm install` could run during the
// installer's registry update. Keep the path construction injectable through
// the env argument for unit tests, but never resolve the Windows executable
// through PATH in production.
function powershellPath(env = process.env) {
  const systemRoot = env.SystemRoot || env.WINDIR;
  if (!systemRoot || !path.win32.isAbsolute(systemRoot)) {
    throw new Error('SystemRoot must be an absolute Windows path');
  }
  return path.win32.join(systemRoot, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
}

function runPowershell(script, extraEnv) {
  const env = extraEnv ? { ...process.env, ...extraEnv } : process.env;
  return execFileSync(powershellPath(env), ['-NoProfile', '-Command', script], {
    encoding: 'utf8',
    env,
  });
}

// Reads a registry string value without OEM-codepage corruption and without %VAR%
// expansion. Stdout carries Base64 (pure ASCII, safe under any console code page)
// instead of the raw value - PowerShell 5.1 writes redirected stdout in the console's
// OEM code page, not UTF-8, which corrupts any non-ASCII character otherwise.
// keyPath/valueName travel via env vars (UTF-16 on Windows) so they're never
// re-encoded either. Exported standalone so tests can hit a scratch key, never Path.
function readRegistryString(keyPath, valueName) {
  const script = [
    '$v = (Get-Item -LiteralPath $env:WIN_NICE_REG_KEY).GetValue(',
    '  $env:WIN_NICE_REG_VALUE, \'\', \'DoNotExpandEnvironmentNames\')',
    '[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes([string]$v))',
  ].join('\n');
  const out = runPowershell(script, { WIN_NICE_REG_KEY: keyPath, WIN_NICE_REG_VALUE: valueName });
  return Buffer.from(out.trim(), 'base64').toString('utf8');
}

// Writes a registry string value, preserving REG_EXPAND_SZ if that was already the
// value's kind (so %VAR% entries like WindowsApps survive a round trip instead of
// being frozen into literals). Creates the key if missing, for scratch-key tests.
function writeRegistryString(keyPath, valueName, value) {
  const script = [
    'if (-not (Test-Path -LiteralPath $env:WIN_NICE_REG_KEY)) {',
    '  New-Item -Path $env:WIN_NICE_REG_KEY -Force | Out-Null',
    '}',
    '$kind = \'String\'',
    'try {',
    '  if ((Get-Item -LiteralPath $env:WIN_NICE_REG_KEY).GetValueKind($env:WIN_NICE_REG_VALUE) -eq [Microsoft.Win32.RegistryValueKind]::ExpandString) {',
    '    $kind = \'ExpandString\'',
    '  }',
    '} catch {}',
    'Set-ItemProperty -LiteralPath $env:WIN_NICE_REG_KEY -Name $env:WIN_NICE_REG_VALUE -Value $env:WIN_NICE_REG_NEW_VALUE -Type $kind',
  ].join('\n');
  runPowershell(script, {
    WIN_NICE_REG_KEY: keyPath,
    WIN_NICE_REG_VALUE: valueName,
    WIN_NICE_REG_NEW_VALUE: value,
  });
}

// A raw registry write (unlike [Environment]::SetEnvironmentVariable) doesn't notify
// running processes. Broadcast WM_SETTINGCHANGE so Explorer/new shells pick it up.
function broadcastEnvironmentChange() {
  const script = [
    'Add-Type -Namespace WinNice -Name NativeMethods -MemberDefinition \'[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)] public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);\'',
    '$result = [UIntPtr]::Zero',
    '[WinNice.NativeMethods]::SendMessageTimeout([IntPtr]0xffff, 0x1A, [UIntPtr]::Zero, "Environment", 2, 5000, [ref]$result) | Out-Null',
  ].join('\n');
  runPowershell(script);
}

const USER_ENV_KEY = 'HKCU:\\Environment';

// Real registry reads/writes - via the registry directly rather than
// [Environment]::...('User') (OEM-codepage + expansion pitfalls, see
// readRegistryString) or `setx` (truncates PATH silently past ~1024 chars).
function readUserPath() {
  return readRegistryString(USER_ENV_KEY, 'Path');
}

function writeUserPath(newPath) {
  writeRegistryString(USER_ENV_KEY, 'Path', newPath);
  broadcastEnvironmentChange();
}

module.exports = {
  isSourceCheckout,
  installRoot,
  binDir,
  manifestPath,
  addToPathString,
  removeFromPathString,
  powershellPath,
  readUserPath,
  writeUserPath,
  readRegistryString,
  writeRegistryString,
};
