// Prepare a manually activated launcher. Never starts or quits the Desktop.
import { constants } from 'node:fs';
import { access, chmod, lstat, mkdir, realpath, writeFile } from 'node:fs/promises';
import { dirname, isAbsolute, join, resolve } from 'node:path';
import { execFileSync } from 'node:child_process';
import { homedir } from 'node:os';

const usage = 'node scripts/prepare-desktop-bridge.mjs --app <Desktop.app> --bridge <bridge binary> --guarded <guarded app-server> --output <new private directory> [--data-dir <AutoResume data directory>]';

function parseArgs(args) {
  const options = {};
  for (let i = 0; i < args.length; i += 2) {
    const key = args[i];
    if (!['--app', '--bridge', '--guarded', '--output', '--data-dir'].includes(key)
        || !args[i + 1] || options[key]) throw new Error(usage);
    options[key] = args[i + 1];
  }
  if (!['--app', '--bridge', '--guarded', '--output'].every(key => options[key])) throw new Error(usage);
  return options;
}

async function executable(path) {
  const resolved = await realpath(path);
  if (!(await lstat(resolved)).isFile()) throw new Error('运行组件必须是普通文件');
  await access(resolved, constants.X_OK);
  return resolved;
}

function plistValue(app, key) {
  try {
    return execFileSync('/usr/libexec/PlistBuddy', ['-c', `Print ${key}`, join(app, 'Contents', 'Info.plist')], {
      encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'],
    }).trim();
  } catch {
    throw new Error('无法核对 Desktop 应用信息');
  }
}

function shellQuote(value) {
  return `'${value.replaceAll("'", "'\\''")}'`;
}

async function privateDirectory(path) {
  await mkdir(path, { recursive: true, mode: 0o700 });
  const metadata = await lstat(path);
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
    throw new Error('AutoResume 数据目录必须是普通目录，不能是符号链接');
  }
  // The app owns this directory. Tighten only this directory, never its parents.
  await chmod(path, 0o700);
  return realpath(path);
}

async function existingAncestor(path) {
  let current = path;
  for (;;) {
    try {
      return await realpath(current);
    } catch (error) {
      if (error.code !== 'ENOENT' || dirname(current) === current) throw error;
      current = dirname(current);
    }
  }
}

async function main() {
  if (process.platform !== 'darwin') {
    throw new Error('仅支持准备 macOS 桌面连接；Windows 原运行环境与沙箱保持尚未验证');
  }
  const options = parseArgs(process.argv.slice(2));
  const app = await realpath(options['--app']);
  if (!app.endsWith('.app') || plistValue(app, 'CFBundleIdentifier') !== 'com.openai.codex') {
    throw new Error('请选择原 Codex Desktop 应用');
  }
  const appExecutableName = plistValue(app, 'CFBundleExecutable');
  if (!appExecutableName || appExecutableName.includes('/') || appExecutableName.includes('\\')) {
    throw new Error('桌面应用启动路径无效');
  }
  const desktop = await executable(join(app, 'Contents', 'MacOS', appExecutableName));
  const officialCli = await executable(join(app, 'Contents', 'Resources', 'codex-cli', 'CodexCLI.app', 'Contents', 'MacOS', 'codex'));
  const bridge = await executable(options['--bridge']);
  const guardedAppServer = await executable(options['--guarded']);
  if (new Set([desktop, officialCli, bridge, guardedAppServer]).size !== 4) {
    throw new Error('运行组件路径重复；拒绝递归或替换原官方文件');
  }
  const cliVersion = execFileSync(officialCli, ['--version'], {
    encoding: 'utf8', timeout: 10_000, stdio: ['ignore', 'pipe', 'pipe'],
  }).trim();
  if (!['codex-cli 0.163.0', 'codex-cli 0.163.0-alpha.10.2'].includes(cliVersion)) {
    throw new Error('此 Desktop 执行协议版本尚未核实，请先更新并验证桥接补丁');
  }
  const build = plistValue(app, 'CFBundleVersion');
  const output = resolve(options['--output']);
  const requestedDataDir = resolve(options['--data-dir']
    ?? join(homedir(), 'Library', 'Application Support', 'io.codex-autoresume.desktop'));
  const outputParent = await realpath(dirname(output));
  const dataAncestor = await existingAncestor(requestedDataDir);
  if (!isAbsolute(output) || outputParent === app || outputParent.startsWith(`${app}/`)
      || dataAncestor === app || dataAncestor.startsWith(`${app}/`)) {
    throw new Error('准备目录不能位于官方应用内部');
  }
  // Exclusive directory creation avoids replacing an active launch generation.
  await mkdir(output, { mode: 0o700 });
  const canonicalOutput = await realpath(output);
  const dataDir = await privateDirectory(requestedDataDir);
  const configPath = join(canonicalOutput, 'bridge.json');
  await writeFile(configPath, `${JSON.stringify({ officialCli, guardedAppServer, dataDir }, null, 2)}\n`, { flag: 'wx', mode: 0o600 });
  const launcher = join(canonicalOutput, '启动桌面续跑.command');
  const script = `#!/bin/sh
set -eu
if [ "$(/usr/libexec/PlistBuddy -c 'Print CFBundleVersion' ${shellQuote(join(app, 'Contents', 'Info.plist'))})" != ${shellQuote(build)} ]; then
  printf '%s\\n' 'Codex Desktop 已更新，请重新核实并准备桌面连接。' >&2
  exit 1
fi
if /usr/bin/pgrep -x ${shellQuote(appExecutableName)} >/dev/null 2>&1; then
  printf '%s\\n' '请先保存工作并完全退出 Codex Desktop，再重新打开此启动器。现有聊天不会被自动停止。' >&2
  exit 1
fi
printf '%s\\n' '正在启动桌面桥接；只有通过连接核对并单独开启的任务才允许自动续跑。'
exec /usr/bin/env CODEX_CLI_PATH=${shellQuote(bridge)} AUTORESUME_BRIDGE_CONFIG=${shellQuote(configPath)} ${shellQuote(desktop)}
`;
  await writeFile(launcher, script, { flag: 'wx', mode: 0o700 });
  const version = plistValue(app, 'CFBundleShortVersionString');
  await writeFile(join(canonicalOutput, '准备说明.txt'), [
    `Desktop 版本：${version}`,
    `执行协议版本：${cliVersion}`,
    '此连接使用 AutoResume 实验性桌面后端，仍需独立验证原聊天续跑。',
    '这只是待手动启用的启动器；本命令没有退出或启动 Desktop。',
    '先完成当前工作并完全退出 Desktop，然后打开“启动桌面续跑.command”。',
    '不会修改原应用、登录凭据、会话或开机启动项。',
    '运行数据保存在 AutoResume 应用数据目录，不放入源码临时目录；测试可显式指定隔离数据目录。',
    '需要单独验证桌面运行组件与权限保持后，才能宣称自动续跑可用。',
    '',
  ].join('\n'), { flag: 'wx', mode: 0o600 });
  console.log('已准备“启动桌面续跑.command”，尚未启用。请在指定输出目录查看准备说明。');
}

main().catch(error => {
  // Filesystem errors can contain personal installation paths.
  console.error(error.code ? `准备失败（${error.code}），请检查路径和权限。` : error.message);
  process.exitCode = 1;
});
