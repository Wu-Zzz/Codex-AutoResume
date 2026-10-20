# Codex AutoResume

为本地 Codex 任务等待额度恢复的轻量桌面助手。面向 macOS 和 Windows，Tauri 2 + Vue 3 + Rust + SQLite。

**Alpha：为从工具启动的新任务提供自动续跑。** 使用方式是：选择项目 → 打开官方 Codex 终端 → 明确开启自动续跑。已有 Desktop / CLI 对话只监测，不能透明接管。

实际执行链路使用独立的官方 Codex 运行目录和存活进程，待额度恢复后在原会话提交继续请求。模拟验证、真实账号只读检查、真实额度恢复、Windows 实机验收分别记录在[验证记录](docs/VALIDATION.md)；实现代码不等于已完成全部实机验收。

![任务列表界面，使用隔离的示例数据](docs/images/app-preview.png)

[Figma 可编辑设计](https://www.figma.com/design/jQ2j1oO7iyMbQOpZIoEgpd?node-id=10-94)包含托盘小窗、任务详情、添加任务和连接引导。应用已扩展双主题与额度卡；Figma 尚未同步这两项。

## 当前实现

- 从工具选择项目，打开官方终端。首次使用由官方 Codex 完成独立登录，不复制现有账号凭据。
- 托管任务有单独的自动续跑授权，默认关闭。旧会话的开关仍只表示监测。
- Rust 解析所有返回的额度桶，只有明确终止的 `usageLimitExceeded` 才允许进入续跑检查。真实窗口、用量和重置时间显示在额度卡中；未知数据不伪造。
- 同一后台处理用户与自动提交，发送前保存意图，结果不明不会盲目重发。多个自动恢复任务串行启动。
- 用户停止、正常结束、审批与补充信息不会触发继续。终端断开、应用重启、配置变化会关闭自动授权；当前不自动冷恢复。
- 主窗口与托盘支持深色、浅色、跟随系统，保持一致并保存偏好。关闭主窗口后继续在托盘运行。

## 下载与 Release

安装包见 [GitHub Releases](https://github.com/Wu-Zzz/Codex-AutoResume/releases)。Mac Apple Silicon / Intel 为 `.dmg`，Windows x64 为 `.exe`（NSIS）。只有三个安装包都通过构建和包装检查，发布流程才会公开该版本。

已通过一次真实隔离任务执行；真实额度耗尽后的自动恢复和 Windows 实机操作仍未验证。请按 Alpha 版本使用，完整范围见[验证记录](docs/VALIDATION.md)。

macOS 目前使用 ad-hoc 签名，未完成 Developer ID 签名和公证；Windows 没有代码签名。应用不附带官方 Codex CLI。防休眠、Git 分支检查、Webhook、自动安装 CLI、自启动暂未实现。

## 运行开发版本

需要 Node.js 22.12+、Rust stable、对应系统的 [Tauri 前置组件](https://v2.tauri.app/start/prerequisites/)，以及已安装的官方 [Codex CLI](https://learn.chatgpt.com/docs/cli)。当前核查版本是 `codex-cli 0.145.0`。应用复用 Codex 自己管理的登录，不读取或保存 Token。

```sh
npm ci
npm run tauri dev
```

Finder 启动会尝试常见安装位置及 nvm 的本地 Node 目录。高级诊断可通过 `CODEX_BIN` 指定官方 Codex 可执行文件；这不是普通用户必填配置。

如果自行把 Rust 安装在忽略的 `work/toolchain/` 目录（`cargo/` 和 `rustup/`）中，可通过辅助脚本启动，不必修改系统 Shell 配置：

```sh
node scripts/local-toolchain.mjs npm run tauri dev
```

仅查看界面：`npm run dev`，打开终端显示的本地地址。默认浏览器页面明确提示需要桌面应用；附加 `?demo=1` 可操作示例任务，**不会调用 Codex**。

## 验证与打包

```sh
npm run check
npm test
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
node scripts/probe-codex.mjs
npm run tauri build
```

`probe-codex.mjs` 只调用握手、账号读取、会话列表、会话读取和额度读取，只输出数量和成功标记，不打印账号身份或聊天内容。它不会恢复、启动、停止或修改任务。

macOS / Windows 的构建结果见 [GitHub Actions](https://github.com/Wu-Zzz/codex-autoresume/actions/workflows/ci.yml)，安装包构建见[发布流程](https://github.com/Wu-Zzz/Codex-AutoResume/actions/workflows/release.yml)。开发者签名、公证、一键安装 Codex、通知、自启动与保持唤醒尚未实现。云端构建、Windows 实机与真实跨额度窗口验证分别记录在 [验证记录](docs/VALIDATION.md)。

## 实现边界

Vue 只渲染状态、偏好与窄接口，Rust 负责调度和持久化。`protocol.rs` 是外部会话的只读适配器；`managed.rs` 是隔离的新任务执行通道，使用带随机凭据的本地回环连接与官方 CLI。只有当前存活进程创建的线程能建立执行所有权，旧监测记录不能变成执行授权。

官方 Codex 自行管理登录。应用不读取、复制或保存账号 Token，不修改 Codex 数据库、日志或配置。临时本地连接随机值只用于本机网关鉴权，不是账号凭据。

数据库仅保存任务标识、标题、目录、状态、授权、配置指纹与发送意图；不保存聊天正文。移除任务不会删除官方 Codex 会话。配置、所有权或发送结果不明时停止自动操作。详见[协议核查](docs/protocol-audit.md)和[产品方案](PLAN.md)。

[Figma](https://www.figma.com/design/jQ2j1oO7iyMbQOpZIoEgpd?node-id=10-94)当前仍为上一版概念稿；额度卡与双主题同步因工具调用上限未完成，实际应用以源码与验证截图为准。

MIT License。独立开源项目，与 OpenAI 无隶属关系。
