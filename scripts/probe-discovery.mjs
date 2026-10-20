import { statSync } from 'node:fs';
import { win32 } from 'node:path';

function isFile(path) {
  try { return statSync(path).isFile(); } catch { return false; }
}

// Resolve Windows npm launchers to their JS entry without invoking cmd or PowerShell.
// Inject filesystem checks so both platform paths can be verified on either runner.
export function codexCommand({ platform = process.platform, env = process.env, node = process.execPath, fileExists = isFile } = {}) {
  const environment = name => {
    const key = Object.keys(env).find(key => key.toUpperCase() === name);
    return key ? env[key] : undefined;
  };
  const override = platform === 'win32' ? environment('CODEX_BIN') : env.CODEX_BIN;
  if (platform !== 'win32') return { command: override || 'codex', args: ['app-server'] };

  const candidates = override ? [override] : (environment('PATH') || '')
    .split(';')
    .map(directory => directory.trim().replace(/^"(.*)"$/, '$1'))
    .filter(Boolean)
    .flatMap(directory => ['codex.exe', 'codex.cmd', 'codex.ps1'].map(name => win32.join(directory, name)));
  if (!override && environment('APPDATA')) candidates.push(win32.join(environment('APPDATA'), 'npm', 'codex.cmd'));
  const executable = candidates.find(fileExists);
  if (!executable) throw new Error('Codex executable not found. Install the official CLI or set CODEX_BIN.');

  if (['.cmd', '.ps1', '.bat'].includes(win32.extname(executable).toLowerCase())) {
    const script = win32.join(win32.dirname(executable), 'node_modules', '@openai', 'codex', 'bin', 'codex.js');
    if (!fileExists(script)) throw new Error('Codex npm entry point is missing. Reinstall the official CLI.');
    return { command: node, args: [script, 'app-server'] };
  }
  return { command: executable, args: ['app-server'] };
}
