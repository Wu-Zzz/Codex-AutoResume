import { describe, expect, it } from 'vitest';
import { codexCommand } from './probe-discovery.mjs';

function windows(env, files) {
  return codexCommand({ platform: 'win32', env, node: 'C:\\Program Files\\nodejs\\node.exe', fileExists: path => files.includes(path) });
}

describe('read-only probe executable discovery', () => {
  it('uses the running Node executable for npm launchers found on Windows Path', () => {
    const directory = 'C:\\Users\\Example & Co\\AppData\\Roaming\\npm';
    const script = `${directory}\\node_modules\\@openai\\codex\\bin\\codex.js`;
    expect(windows({ Path: `C:\\empty;"${directory}"` }, [`${directory}\\codex.cmd`, script])).toEqual({
      command: 'C:\\Program Files\\nodejs\\node.exe', args: [script, 'app-server'],
    });
  });

  it('finds the Windows npm directory when it is absent from PATH', () => {
    const directory = 'C:\\Users\\Example\\AppData\\Roaming\\npm';
    const script = `${directory}\\node_modules\\@openai\\codex\\bin\\codex.js`;
    expect(windows({ APPDATA: 'C:\\Users\\Example\\AppData\\Roaming' }, [`${directory}\\codex.cmd`, script]).args).toEqual([script, 'app-server']);
  });

  it('honors an explicit native binary without falling back to another installation', () => {
    const command = 'D:\\Codex Tools\\codex.exe';
    expect(windows({ CODEX_BIN: command, PATH: 'C:\\other' }, [command, 'C:\\other\\codex.exe'])).toEqual({ command, args: ['app-server'] });
    expect(() => windows({ CODEX_BIN: command, PATH: 'C:\\other' }, ['C:\\other\\codex.exe'])).toThrow('executable not found');
  });

  it('resolves explicit PowerShell launchers and refuses a missing npm entry point', () => {
    const command = 'D:\\npm\\Codex.PS1';
    const script = 'D:\\npm\\node_modules\\@openai\\codex\\bin\\codex.js';
    expect(windows({ CODEX_BIN: command }, [command, script]).args).toEqual([script, 'app-server']);
    expect(() => windows({ CODEX_BIN: command }, [command])).toThrow('entry point is missing');
  });

  it('preserves POSIX executable lookup and literal explicit paths', () => {
    expect(codexCommand({ platform: 'darwin', env: {} })).toEqual({ command: 'codex', args: ['app-server'] });
    const command = '/Applications/Codex Tools/codex';
    expect(codexCommand({ platform: 'darwin', env: { CODEX_BIN: command } })).toEqual({ command, args: ['app-server'] });
  });
});
