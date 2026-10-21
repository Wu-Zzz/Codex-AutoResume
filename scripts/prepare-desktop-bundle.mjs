// Packages only the explicitly modified runtime; never starts Codex or reads an account.
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { access, chmod, copyFile, lstat, mkdir, readFile, writeFile } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { dirname, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

const root = fileURLToPath(new URL('../', import.meta.url));
const integration = join(root, 'integrations/codex');
const files = ['codex-app-server', 'LICENSE', 'NOTICE', 'upstream.json', 'MODIFICATIONS.txt', 'build-info.json'];
const { positionals, values } = parseArgs({
  allowPositionals: true,
  options: Object.fromEntries(['backend', 'runtime', 'output', 'target', 'config', 'app'].map(key => [key, { type: 'string' }])),
});
const [mode] = positionals;
const architecture = { 'aarch64-apple-darwin': 'arm64', 'x86_64-apple-darwin': 'x86_64' }[values.target];
if (positionals.length !== 1 || !['stage', 'prepare', 'verify'].includes(mode) || !architecture || process.platform !== 'darwin') {
  throw new Error('Usage on macOS: prepare-desktop-bundle.mjs stage --backend FILE --output DIR --target TARGET | prepare --runtime DIR --config FILE --target TARGET | verify --runtime DIR --app APP --target TARGET');
}
const required = name => {
  if (!values[name]) throw new Error(`Missing --${name}`);
  return resolve(values[name]);
};
const ownedOutput = path => {
  const rel = relative(join(root, 'work'), path);
  if (!rel || rel.startsWith(`..${sep}`) || rel === '..' || resolve(root, 'work', rel) !== path) {
    throw new Error('Generated output must be inside this checkout\'s work/ directory');
  }
  return path;
};
const revision = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim();
const manifest = JSON.parse(await readFile(join(integration, 'upstream.json'), 'utf8'));
async function digest(path) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest('hex');
}
async function regular(path, executable = false) {
  const info = await lstat(path);
  if (!info.isFile() || info.isSymbolicLink() || (executable && !(info.mode & 0o111))) {
    throw new Error(`Expected ${executable ? 'executable ' : ''}regular file: ${path}`);
  }
}
async function binary(path) {
  await regular(path, true);
  const arch = execFileSync('lipo', ['-archs', path], { encoding: 'utf8' }).trim();
  if (arch !== architecture) throw new Error(`Wrong architecture for ${path}: ${arch}`);
}
async function validateRuntime(runtime) {
  for (const name of files) await regular(join(runtime, name));
  await binary(join(runtime, 'codex-app-server'));
  for (const name of ['LICENSE', 'NOTICE', 'upstream.json']) {
    if (await digest(join(runtime, name)) !== await digest(join(integration, name))) throw new Error(`Runtime ${name} differs from the checked-out source`);
  }
  const info = JSON.parse(await readFile(join(runtime, 'build-info.json'), 'utf8'));
  if (info.sourceCommit !== revision || info.target !== values.target ||
      info.upstreamArchiveSha256 !== manifest.archiveSha256 ||
      info.backendSha256 !== await digest(join(runtime, 'codex-app-server'))) {
    throw new Error('Runtime provenance or binary checksum does not match this release');
  }
  const modifications = await readFile(join(runtime, 'MODIFICATIONS.txt'), 'utf8');
  if (!modifications.includes('unofficial modified build') || !modifications.includes(revision)) throw new Error('Missing modified-build notice');
}

if (mode === 'stage') {
  const backend = required('backend');
  const output = ownedOutput(required('output'));
  await binary(backend);
  await mkdir(output, { recursive: true });
  if (backend !== join(output, 'codex-app-server')) await copyFile(backend, join(output, 'codex-app-server'));
  await chmod(join(output, 'codex-app-server'), 0o755);
  for (const name of ['LICENSE', 'NOTICE', 'upstream.json']) await copyFile(join(integration, name), join(output, name));
  await writeFile(join(output, 'MODIFICATIONS.txt'), [
    'Codex AutoResume: unofficial modified build of Codex App Server.',
    `Upstream: ${manifest.repository}/tree/${manifest.tag}`,
    `Source and modifications: https://github.com/Wu-Zzz/Codex-AutoResume/tree/${revision}/integrations/codex`,
    `Patches: ${manifest.patches.join(', ')}`,
    'Adds guarded conditional continuation and forwards sandbox execution to the installed official CLI.',
    'Not an official OpenAI distribution. The official code-mode helper is not included.',
    '',
  ].join('\n'));
  await writeFile(join(output, 'build-info.json'), JSON.stringify({
    sourceCommit: revision, target: values.target, upstreamArchiveSha256: manifest.archiveSha256,
    backendSha256: await digest(join(output, 'codex-app-server')),
  }, null, 2) + '\n');
  await validateRuntime(output);
  console.log(`Staged modified runtime for ${values.target}: ${output}`);
} else {
  const runtime = required('runtime');
  await validateRuntime(runtime);
  if (mode === 'prepare') {
    const config = ownedOutput(required('config'));
    await mkdir(dirname(config), { recursive: true });
    await writeFile(config, JSON.stringify({ bundle: { resources: Object.fromEntries(files.map(name => [join(runtime, name), `desktop/${name}`])) } }, null, 2) + '\n');
    console.log(`Prepared Tauri resource configuration: ${config}`);
  } else {
    const app = required('app');
    await binary(join(app, 'Contents/MacOS/codex-autoresume'));
    await binary(join(app, 'Contents/MacOS/codex-autoresume-bridge'));
    const bundled = join(app, 'Contents/Resources/desktop');
    await validateRuntime(bundled);
    for (const name of files) {
      if (await digest(join(runtime, name)) !== await digest(join(bundled, name))) throw new Error(`Bundled ${name} differs from the verified artifact`);
    }
    try {
      await access(join(bundled, 'codex-code-mode-host'));
      throw new Error('The official code-mode helper must not be redistributed');
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
    }
    execFileSync('codesign', ['--verify', '--deep', '--strict', app], { stdio: 'inherit' });
    const help = execFileSync(join(bundled, 'codex-app-server'), ['--help'], { encoding: 'utf8', timeout: 10_000 });
    if (!help.trim()) throw new Error('Bundled backend did not return help output');
    console.log(`Verified app architecture, executable permissions, runtime provenance and signature: ${app}`);
  }
}
