// Optional project-local Rust setup, without changing shell profiles or global PATH.
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { resolve, delimiter } from 'node:path';
const root = fileURLToPath(new URL('../', import.meta.url));
const cargo = resolve(root, 'work/toolchain/cargo');
const rustup = resolve(root, 'work/toolchain/rustup');
if (!existsSync(resolve(cargo, 'bin'))) throw new Error('Project-local Rust is not installed. Use your system toolchain instead.');
const [program, ...args] = process.argv.slice(2);
if (!program) throw new Error('Pass a command, e.g. node scripts/local-toolchain.mjs npm run tauri dev');
const child = spawn(program, args, { cwd: root, stdio: 'inherit', env: { ...process.env, CARGO_HOME: cargo, RUSTUP_HOME: rustup, PATH: `${resolve(cargo, 'bin')}${delimiter}${process.env.PATH ?? ''}` } });
child.on('error', error => { console.error(error.message); process.exitCode = 1; });
child.on('exit', code => { process.exitCode = code ?? 1; });
