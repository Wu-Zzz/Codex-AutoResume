# 原 Desktop 聊天续跑（实验）

这条通路保留原 Codex Desktop、原聊天 ID、历史上下文与原客户端工具连接。用户不需要把聊天迁到终端。当前只提供 macOS 开发集成；不自动替换官方应用，不修改官方数据库、凭据或 rollout。

## 组成

- 控制台桥接器承接 Desktop 的 stdio 连接。普通 CLI 命令转给原官方程序；桌面消息、审批和动态工具仍由原客户端处理。
- [执行端补丁](../integrations/codex/README.md)基于官方 0.163.0 源码，新增“仅当指定额度失败仍是最新轮次且空闲才继续”的原子检查。它是明确标识的修改版，并非官方发布功能。
- Rust 调度器在真实额度放行后调用这项条件操作，发送前保存不可重复的意图。未知协议或提交结果不明时停止，不退回普通 `turn/start`。
- AutoResume 显示当前桥接中已打开的聊天，开关默认关闭。工具调用、权限审批和用户输入不会转移到 AutoResume 界面。

## 构建和准备

以下操作只构建并生成待手动打开的启动器，不退出或启动用户的 Desktop：

```sh
npm ci
npm run build
cargo build --manifest-path src-tauri/Cargo.toml --bins
node scripts/build-guarded-codex.mjs --test

node scripts/prepare-desktop-bridge.mjs \
  --app /Applications/ChatGPT.app \
  --bridge "$PWD/src-tauri/target/debug/codex-autoresume-bridge" \
  --guarded "$PWD/work/guarded-codex-build/bin/codex-app-server" \
  --output "$PWD/work/desktop-launcher"
```

`--app` 指向本机真实的 Codex Desktop 应用；目录名可能因安装版本不同而不同。输出目录必须尚不存在，避免覆盖正在使用的配置。准备脚本会校验路径与私有文件权限。

原 Desktop 完全退出后，再手动打开生成的 `启动桌面续跑.command`；启动器不会强制结束已有进程。打开本次构建的 AutoResume，在原 Desktop 中进入需要守护的聊天，然后在 AutoResume 对该聊天单独开启续跑。新消息或停止会撤销这次许可，需要时重新开启。

恢复普通使用：完全退出实验连接中的 Desktop，再通过平时的图标打开原应用。没有修改官方程序，也没有设置全局环境变量或开机启动项。

## 当前边界

- 必须在本次连接里实际观察到终态 `usageLimitExceeded`，并保存原生轮次的执行设置。重启以前的失败不能直接当作执行授权。
- 必须取得新版接口 `ordinaryUsageAllowed = true`，并且全部已返回额度桶通过检查。倒计时到点、剩余额度百分比或可购买余额都不能代替此条件。
- 用户停止、输入新任务、改变配置、等待审批、需要回答问题或出现未知协议，会关闭该聊天的自动续跑。正常结束不会催促。
- 原客户端仍处理工具请求。桥接不合成批准、回答、凭据或工具结果。
- 同一失败只允许一次提交。断线、超时或确认丢失后不自动重发。重启桥接不会继承旧开关。
- 桌面控制连接消失会撤销许可；只关闭 AutoResume 窗口仍由 Rust 后台维持守护。完全退出 AutoResume 会撤销续跑，不停止正在执行的原聊天。
- 已知重复桥接实例会阻止自动提交。未经过桥接的其他客户端不受本工具控制；本模式不能作为任意客户端之间的全局独占锁。
- Windows 自定义 CLI 路径会影响原运行环境和沙箱设置，当前明确拒绝激活，不能以编译成功代替实机验证。
- 没有防休眠功能。真实额度耗尽/恢复、桌面专属工具、Mac Intel 和 Windows 的实际使用仍需分别验收。

当前验证证据见 [VALIDATION.md](VALIDATION.md)。模拟后端通过、协议结构一致与成功编译，不代表真实 Desktop 已完成验收。
