// Compiles only the std-only forwarding module. No real Codex or account is used.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmod, link, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const source = process.argv[2];
if (!source || process.argv.length !== 3) {
  throw new Error('Usage: node scripts/test-guarded-sandbox.mjs <prepared codex-rs directory>');
}
const modulePath = resolve(source, 'app-server/src/autoresume_sandbox.rs');
const work = join(root, 'work/guarded-sandbox-tests');
await mkdir(work, { recursive: true });
const temp = await mkdtemp(join(work, 'case-'));
const executable = join(temp, process.platform === 'win32' ? 'dispatch.exe' : 'dispatch');
const harness = join(temp, 'dispatch.rs');
const argvRecord = join(temp, 'argv');
const stdinRecord = join(temp, 'stdin');
const envRecord = join(temp, 'env');
let checks = 0;

function run(args, overrides = {}) {
  const env = {
    ...process.env,
    SANDBOX_TEST_ARGV: argvRecord,
    SANDBOX_TEST_STDIN: stdinRecord,
    SANDBOX_TEST_ENV: envRecord,
    CODEX_HOME: '/test/unchanged-codex-home',
    CODEX_CLI_PATH: '/test/unchanged-cli-override',
    SANDBOX_TEST_EXIT: '37',
    ...overrides,
  };
  if (overrides.AUTORESUME_OFFICIAL_CLI === undefined) delete env.AUTORESUME_OFFICIAL_CLI;
  const result = spawnSync(executable, args, {
    env, cwd: temp, input: 'untouched stdin\n', encoding: 'utf8', timeout: 10_000,
  });
  assert.ifError(result.error);
  return result;
}

async function rejected(target) {
  await rm(argvRecord, { force: true });
  const result = run(['sandbox', '--help'], { AUTORESUME_OFFICIAL_CLI: target });
  assert.equal(result.status, 70);
  assert.equal(result.stdout, '');
  assert.match(result.stderr, /AutoResume sandbox/);
  await assert.rejects(readFile(argvRecord), { code: 'ENOENT' });
  checks += 1;
}

try {
  // JSON escaping yields a valid Rust UTF-8 path literal on the supported hosts.
  await writeFile(harness, `#[path = ${JSON.stringify(modulePath)}]\nmod autoresume_sandbox;\nfn main() {\n    if let Err(error) = autoresume_sandbox::forward_if_sandbox() {\n        eprintln!("{error}");\n        std::process::exit(70);\n    }\n    println!("STANDALONE");\n    std::process::exit(23);\n}\n`);
  const compile = spawnSync(process.env.RUSTC ?? 'rustc', [
    '--edition=2024', '-D', 'warnings', harness, '-o', executable,
  ], { encoding: 'utf8', timeout: 60_000 });
  assert.ifError(compile.error);
  assert.equal(compile.status, 0, compile.stderr);

  // Unknown or absent forwarding config never affects normal server/arg0 args.
  for (const args of [[], ['--stdio'], ['app-server'], ['-c', 'sandbox'], ['--', 'sandbox'], ['--codex-exec-helper']]) {
    const result = run(args, { AUTORESUME_OFFICIAL_CLI: '/missing/configured/cli' });
    assert.equal(result.status, 23);
    assert.equal(result.stdout, 'STANDALONE\n');
    assert.equal(result.stderr, '');
    checks += 1;
  }

  if (process.platform === 'win32') {
    await rejected(executable);
    console.log(`Sandbox forwarding: ${checks} isolated checks passed; Windows forwarding deliberately refused.`);
  } else {
    const official = join(temp, 'official cli');
    await writeFile(official, `#!/bin/sh\nprintf '%s\\0' "$@" > "$SANDBOX_TEST_ARGV"\ncat > "$SANDBOX_TEST_STDIN"\nprintf '%s\\0' "$CODEX_HOME" "$CODEX_CLI_PATH" "$PWD" > "$SANDBOX_TEST_ENV"\nprintf 'official stdout\\n'\nprintf 'official stderr\\n' >&2\nif [ "$SANDBOX_TEST_EXIT" = signal ]; then kill -TERM "$$"; fi\nexit "$SANDBOX_TEST_EXIT"\n`, { mode: 0o700 });
    const args = ['sandbox', '--sandbox-state-json', '{}', '--', '', '空 格', "quotes'\"", 'line\nbreak', '$(touch should-not-exist)', '; exit 0'];
    const forwarded = run(args, { AUTORESUME_OFFICIAL_CLI: official });
    assert.equal(forwarded.status, 37);
    assert.equal(forwarded.stdout, 'official stdout\n');
    assert.equal(forwarded.stderr, 'official stderr\n');
    assert.deepEqual((await readFile(argvRecord, 'utf8')).split('\0').slice(0, -1), args);
    assert.equal(await readFile(stdinRecord, 'utf8'), 'untouched stdin\n');
    // PWD can be canonicalized by /bin/sh (macOS /var is a symlink); use real cwd.
    const inherited = (await readFile(envRecord, 'utf8')).split('\0');
    assert.deepEqual(inherited.slice(0, 2), ['/test/unchanged-codex-home', '/test/unchanged-cli-override']);
    assert.equal(resolve(inherited[2]), resolve(temp));
    await assert.rejects(readFile(join(temp, 'should-not-exist')), { code: 'ENOENT' });
    checks += 1;

    await rejected(undefined);
    await rejected('relative-cli');
    await rejected(join(temp, 'missing-cli'));
    await rejected(temp);
    await chmod(official, 0o600);
    await rejected(official);
    await chmod(official, 0o700);
    await rejected(executable);
    const sameInode = join(temp, 'self-hardlink');
    await link(executable, sameInode);
    await rejected(sameInode);
    const selfSymlink = join(temp, 'self-symlink');
    await symlink(executable, selfSymlink);
    await rejected(selfSymlink);

    const officialSymlink = join(temp, 'official-symlink');
    await symlink(official, officialSymlink);
    const throughLink = run(['sandbox', '--help'], { AUTORESUME_OFFICIAL_CLI: officialSymlink });
    assert.equal(throughLink.status, 37);
    assert.deepEqual((await readFile(argvRecord, 'utf8')).split('\0').slice(0, -1), ['sandbox', '--help']);
    checks += 1;

    const signal = run(['sandbox', '--help'], {
      AUTORESUME_OFFICIAL_CLI: official, SANDBOX_TEST_EXIT: 'signal',
    });
    assert.equal(signal.signal, 'SIGTERM');
    assert.equal(signal.status, null);
    assert.doesNotMatch(signal.stdout, /STANDALONE/);
    checks += 1;
    console.log(`Sandbox forwarding: ${checks} isolated checks passed (argv, stdio, environment, exit/signal, invalid targets and recursion).`);
  }
} finally {
  await rm(temp, { recursive: true, force: true });
}
