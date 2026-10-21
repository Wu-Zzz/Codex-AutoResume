# 原 Desktop 聊天续跑（实验）

这条实验通路旨在保留原 Codex Desktop、原聊天 ID、历史上下文与原客户端工具连接，不迁到终端。正常图标的一次性接入已编码，限定 macOS 默认 zsh 环境及 Desktop 构建 `20770`。**原 Desktop 实机与真实额度恢复尚未验收，新 Release 尚未发布。** 不替换官方应用，不修改官方数据库、凭据或 rollout。

## 组成

- 控制台桥接器承接 Desktop 的 stdio 连接。普通 CLI 命令转给原官方程序；桌面消息、审批和动态工具仍由原客户端处理。
- [执行端补丁](../integrations/codex/README.md)基于官方 0.163.0 源码，新增“仅当指定额度失败仍是最新轮次且空闲才继续”的原子检查。它是明确标识的修改版，并非官方发布功能。
- Rust 调度器在真实额度放行后调用这项条件操作，发送前保存不可重复的意图。未知协议或提交结果不明时停止，不退回普通 `turn/start`。
- AutoResume 显示当前桥接中已打开的聊天，开关默认关闭。工具调用、权限审批和用户输入不会转移到 AutoResume 界面。

## 一次性接入（开发版本）

1. 在附有完整实验组件的 AutoResume 中打开 **Codex 连接 → 自动连接 Codex → 开启自动连接**。
2. 首次设置后，正常退出 Codex，再从平时的 Dock / Applications 图标打开。以后无需专用启动器。
3. 在原 Codex 中打开需要守护的聊天，再在 AutoResume 对该聊天单独开启续跑。接入安装不代表聊天已授权，也不代表已连接成功。

接入在用户 `.zshrc` 中添加可移除的条件入口，仅在 Desktop 的 `CODEX_SHELL=1` 环境下生效；组件保存于 AutoResume 私有目录，不设置系统全局环境或登录服务。点击 **关闭自动连接** 可撤销设置；后续启动恢复官方 CLI。保留已有自定义 CLI 配置，冲突时拒绝启用。组件缺失、配置无效或 Desktop 版本变化时，在执行端启动前回退官方 CLI；已启动的执行端失败不会再启动第二个后端。

已发布公共安装包未附完整执行端。当前源码遇到组件不完整会禁用入口。本地 debug 版本可使用下面生成的 `work/guarded-codex-build/bin/codex-app-server`；这不代表公开安装包已具备或通过桌面续跑验收。

## 开发构建

以下命令只构建组件，不退出或启动用户的 Desktop：

```sh
npm ci
npm run build
cargo build --manifest-path src-tauri/Cargo.toml --bins
node scripts/build-guarded-codex.mjs --test
```

运行本次构建的 debug 应用后，通过上述接入入口准备。修改版执行端的来源和许可证见[执行端补丁](../integrations/codex/README.md)。

### 高级诊断：独立启动器

旧独立脚本仅用于开发诊断，不是日常启动方式。它生成一个待手动运行的入口，不安装正常图标接入：

```sh
node scripts/prepare-desktop-bridge.mjs \
  --app /Applications/ChatGPT.app \
  --bridge "$PWD/src-tauri/target/debug/codex-autoresume-bridge" \
  --guarded "$PWD/work/guarded-codex-build/bin/codex-app-server" \
  --output "$PWD/work/desktop-launcher"
```

`--app` 指向本机真实的 Codex Desktop 应用；目录名可能因安装版本不同而不同。输出目录必须尚不存在，避免覆盖正在使用的配置。准备脚本会校验路径与私有文件权限。

原 Desktop 完全退出后，才可手动打开生成的 `启动桌面续跑.command`；脚本不会强制结束已有进程。此诊断入口与普通图标接入分开，聊天仍需单独授权。

诊断结束后完全退出 Desktop；若曾启用正常图标接入，先在 AutoResume 中关闭接入，再从原图标启动即可使用官方执行端。

## 当前边界

- 必须在本次连接里实际观察到终态 `usageLimitExceeded`，并保存原生轮次的执行设置。重启以前的失败不能直接当作执行授权。
- 必须取得新版接口 `ordinaryUsageAllowed = true`，并且全部已返回额度桶通过检查。倒计时到点、剩余额度百分比或可购买余额都不能代替此条件。
- 用户停止、输入新任务、改变配置、等待审批、需要回答问题或出现未知协议，会关闭该聊天的自动续跑。正常结束不会催促。
- 原客户端仍处理工具请求。桥接不合成批准、回答、凭据或工具结果。
- 同一失败只允许一次提交。断线、超时或确认丢失后不自动重发。重启桥接不会继承旧开关。
- 桌面控制连接消失会撤销许可；只关闭 AutoResume 窗口仍由 Rust 后台维持守护。完全退出 AutoResume 会撤销续跑，不停止正在执行的原聊天。
- 已知重复桥接实例会阻止自动提交。未经过桥接的其他客户端不受本工具控制；本模式不能作为任意客户端之间的全局独占锁。
- Windows 自定义 CLI 路径会影响原运行环境和沙箱设置，当前明确拒绝激活，不能以编译成功代替实机验证。
- 当前修改版执行程序未配套兼容的语音组件，不支持 Desktop 语音功能。
- 没有防休眠功能。真实额度耗尽/恢复、桌面专属工具、Mac Intel 和 Windows 的实际使用仍需分别验收。

当前验证证据见 [VALIDATION.md](VALIDATION.md)。模拟后端通过、协议结构一致与成功编译，不代表真实 Desktop 已完成验收。
