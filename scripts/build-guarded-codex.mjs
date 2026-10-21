// Builds the explicitly modified upstream runtime; never starts an App Server.
import { createHash } from 'node:crypto';
import { createReadStream, createWriteStream } from 'node:fs';
import { access, copyFile, mkdir, mkdtemp, open, readFile, rename, rm } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { resolve, dirname, join } from 'node:path';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const integration = join(root, 'integrations/codex');
const work = join(root, 'work/guarded-codex-build');
const flags = new Set(process.argv.slice(2));
const allowedFlags = new Set(['--test', '--prepare-only', '--help']);
if ([...flags].some(flag => !allowedFlags.has(flag)) ||
    (flags.has('--test') && flags.has('--prepare-only'))) {
  console.error('Usage: node scripts/build-guarded-codex.mjs [--test | --prepare-only]');
  process.exit(2);
}
if (flags.has('--help')) {
  console.log(`Build the modified Codex runtime from a checksum-pinned official source archive.

  node scripts/build-guarded-codex.mjs                 Build only
  node scripts/build-guarded-codex.mjs --test          Build and run isolated mock tests
  node scripts/build-guarded-codex.mjs --prepare-only  Verify, extract and apply patches only

Requires Node >=20, Git, tar, and Rust 1.95.0 with native build prerequisites.
Uses ignored work/guarded-codex-build; no account, client, configuration or
installation is changed. The runtime is modified and is not an official build.`);
  process.exit(0);
}

function run(program, args, options = {}) {
  return new Promise((resolveResult, reject) => {
    const child = spawn(program, args, {
      cwd: options.cwd ?? root,
      env: options.env ?? process.env,
      stdio: options.capture ? ['ignore', 'pipe', 'inherit'] : 'inherit',
      windowsHide: true,
    });
    let output = '';
    if (options.capture) child.stdout.setEncoding('utf8').on('data', text => { output += text; });
    child.on('error', error => reject(new Error(`${program}: ${error.message}`)));
    child.on('exit', code => code === 0
      ? resolveResult(output.trim())
      : reject(new Error(`${program} exited with ${code ?? 'a signal'}`)));
  });
}

async function sha256(path) {
  const digest = createHash('sha256');
  for await (const data of createReadStream(path)) digest.update(data);
  return digest.digest('hex');
}

async function verify(path, expected) {
  const actual = await sha256(path);
  if (actual !== expected) throw new Error(`Checksum mismatch: ${path}\nExpected ${expected}\nActual   ${actual}`);
}

async function exists(path) {
  try { await access(path); return true; } catch { return false; }
}

async function getArchive(manifest) {
  const archive = join(work, `${manifest.tag}.tar.gz`);
  if (await exists(archive)) {
    await verify(archive, manifest.archiveSha256);
    return archive;
  }
  // Reuse the development download only when its exact bytes match the pin.
  const developmentArchive = join(root, 'work/codex-guard/source.tar.gz');
  if (await exists(developmentArchive) && await sha256(developmentArchive) === manifest.archiveSha256) {
    await copyFile(developmentArchive, archive);
    return archive;
  }
  const partial = `${archive}.part`;
  const response = await fetch(manifest.archiveUrl, {
    redirect: 'follow',
    signal: AbortSignal.timeout(120_000),
  });
  if (!response.ok || !response.body) throw new Error(`Source download failed: HTTP ${response.status}`);
  try {
    await pipeline(Readable.fromWeb(response.body), createWriteStream(partial, { flags: 'wx' }));
    await verify(partial, manifest.archiveSha256);
    await rename(partial, archive);
  } finally {
    await rm(partial, { force: true });
  }
  return archive;
}

// The release tar has 0.163.0 manifests but 0.0.0 path-package lock versions.
// Prove the supplied repair changes only those local versions, not resolution.
function verifyLockRepair(before, after, manifest) {
  const packages = text => text.split(/(?=^\[\[package\]\])/m).slice(1);
  const prior = packages(before);
  const next = packages(after);
  if (prior.length !== next.length) throw new Error('Lock repair changed package count');
  let changes = 0;
  for (let index = 0; index < prior.length; index += 1) {
    if (prior[index] === next[index]) continue;
    if (/^source = /m.test(prior[index]) ||
        prior[index].replace(/^version = "0\.0\.0"$/m, 'version = "0.163.0"') !== next[index]) {
      throw new Error('Lock repair changed a dependency or a nonlocal package');
    }
    changes += 1;
  }
  if (changes !== manifest.lockfile.changedLocalPackageVersions) {
    throw new Error(`Unexpected lock repair count: ${changes}`);
  }
}

async function main() {
  if (Number(process.versions.node.split('.')[0]) < 20) throw new Error('Node.js 20 or later is required');
  const manifest = JSON.parse(await readFile(join(integration, 'upstream.json'), 'utf8'));
  await run('git', ['--version']);
  await run('tar', ['--version']);
  await mkdir(work, { recursive: true });
  const lockPath = join(work, 'build.lock');
  const lock = await open(lockPath, 'wx').catch(() => {
    throw new Error(`Another build owns ${lockPath}. If a build crashed, confirm it stopped before removing that lock.`);
  });
  let source;
  try {
    await lock.writeFile(`${process.pid}\n`);
    const archive = await getArchive(manifest);
    source = await mkdtemp(join(work, 'source-'));
    await run('tar', ['-xzf', archive, '-C', source, '--strip-components=1']);
    const lockfilePath = join(source, manifest.lockfile.path);
    await verify(lockfilePath, manifest.lockfile.beforeSha256);
    const beforeLock = await readFile(lockfilePath, 'utf8');
    const patchEnv = { ...process.env, GIT_CEILING_DIRECTORIES: work };
    delete patchEnv.GIT_DIR;
    delete patchEnv.GIT_WORK_TREE;
    delete patchEnv.GIT_INDEX_FILE;
    for (const patch of manifest.patches) {
      const path = resolve(integration, patch);
      if (dirname(path) !== integration) throw new Error('Patch path must stay within integrations/codex');
      await run('git', ['apply', '--check', path], { cwd: source, env: patchEnv });
      await run('git', ['apply', path], { cwd: source, env: patchEnv });
    }
    await verify(lockfilePath, manifest.lockfile.afterSha256);
    verifyLockRepair(beforeLock, await readFile(lockfilePath, 'utf8'), manifest);
    console.log(`Verified ${manifest.tag}, applied ${manifest.patches.length} pinned integration patches.\nPrepared source: ${source}`);
    if (flags.has('--prepare-only')) return;

    const workspace = join(source, 'codex-rs');
    const compiler = await run('rustc', ['--version'], { cwd: workspace, capture: true });
    if (!compiler.startsWith(`rustc ${manifest.rustToolchain} `)) {
      throw new Error(`Required Rust ${manifest.rustToolchain}; found ${compiler}`);
    }
    await run('cargo', ['--version'], { cwd: workspace });
    if (flags.has('--test')) {
      await run(process.execPath, [join(root, 'scripts/test-guarded-sandbox.mjs'), workspace], { cwd: workspace });
    }
    const env = {
      ...process.env,
      CARGO_TARGET_DIR: join(work, 'target'),
      CARGO_BUILD_JOBS: '2',
      CARGO_INCREMENTAL: '0',
      CARGO_PROFILE_DEV_DEBUG: '0',
      CARGO_PROFILE_TEST_DEBUG: '0',
    };
    await run('cargo', ['build', '--locked', '-p', 'codex-app-server', '--bin', 'codex-app-server'], { cwd: workspace, env });
    if (flags.has('--test')) {
      const tests = [
        ['-p', 'codex-app-server', '--lib', 'autoresume'],
        ['-p', 'codex-app-server', '--bin', 'codex-app-server', 'app_server_accepts_desktop_launch_arguments_without_changing_defaults'],
        ['-p', 'codex-app-server', '--test', 'all', 'suite::v2::autoresume'],
        ['-p', 'codex-core', '--lib', 'autoresume_continuation_rejects_internal_start_and_completed_successor'],
      ];
      for (const args of tests) await run('cargo', ['test', '--locked', ...args], { cwd: workspace, env });
    }
    const executableName = process.platform === 'win32' ? 'codex-app-server.exe' : 'codex-app-server';
    const output = join(work, 'bin');
    await mkdir(output, { recursive: true });
    await copyFile(join(work, 'target/debug', executableName), join(output, executableName));
    await copyFile(join(integration, 'LICENSE'), join(output, 'LICENSE'));
    await copyFile(join(integration, 'NOTICE'), join(output, 'NOTICE'));
    console.log(`Built modified, unofficial runtime: ${join(output, executableName)}\nNo server was started; no Desktop setting was changed.`);
  } finally {
    await lock.close();
    await rm(lockPath, { force: true });
  }
}

main().catch(error => {
  console.error(error.message);
  process.exitCode = 1;
});
