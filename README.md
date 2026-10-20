# Codex AutoResume

为本地 Codex 任务等待额度恢复的轻量桌面助手。面向 macOS 和 Windows，Tauri 2 + Vue 3 + Rust + SQLite。

**当前是监测原型，核心自动续跑功能尚未完成。** 本地会话发现、历史轮次监测、任务管理和托盘已经实现。额度接口已接通，但返回数据尚未解析并接入实际调度；恢复会话、提交继续消息、处理执行事件的生产链路也尚未实现。本版本**不会自动发送“继续”**。

跨客户端安全接管和原权限保留仍有协议层面的限制，但这不是全部缺口：由本应用独占管理会话的执行方案也还没有实现。不能用它无人值守接管正在运行的 Desktop 或 CLI 任务。

![任务列表界面，使用隔离的示例数据](docs/images/app-preview.png)

[Figma 可编辑设计](https://www.figma.com/design/jQ2j1oO7iyMbQOpZIoEgpd?node-id=4-223)包含主窗口、添加任务和空状态。采用 macOS 原生极简方向；设计中的任务与连接状态均为示例。

## 当前功能

- 发现本机官方 Codex，读取 ChatGPT 登录状态、会话列表和已有轮次记录。额度查询目前仅检查接口是否可用，不提供真实额度倒计时或恢复判定。
- 选择任务、开启或关闭监测、暂停全部、移除本工具的监测记录、查看本工具状态与简短记录。
- 关闭窗口后留在菜单栏或系统托盘；退出菜单会明确停止后台监测；重复打开只激活现有窗口。
- SQLite 保存任务和暂停状态。调度与存储库另有额度判断、原子发送意图、重启去重等规则及模拟测试；生产运行时尚未接入真实发送链路。
- 请求超时、未知状态、审批、用户停止、正常结束及不明确的错误不会触发自动续跑。

主界面的开关控制监测，不能授权执行 Codex 任务。未来版本接入自动执行时，必须另行取得明确授权，不能把已有监测选择当作续跑授权。浏览器演示数据与真实账号完全隔离。

## 下载与 Release

截至 2026-10-20，GitHub 尚无 Release。现有本地开发产物是 macOS `.app` 的 ZIP，不是正式安装包。GitHub Actions 目前使用 `--no-bundle`，只做编译验证。

后续分发格式计划为 macOS `.dmg` 和 Windows `.exe`（NSIS）。当前没有 `.pkg` 发布方案；安装包、签名、公证、安装验证和发布流程尚未完成。

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

macOS / Windows 的构建结果见 [GitHub Actions](https://github.com/Wu-Zzz/codex-autoresume/actions/workflows/ci.yml)。签名、公证、一键安装 Codex、通知、自启动与保持唤醒尚未实现。云端构建、Windows 实机与真实跨额度窗口验证分别记录在 [验证记录](docs/VALIDATION.md)。

## 实现边界

Vue 只调用窄接口，Rust 是状态的唯一来源。`protocol.rs` 对 RPC 方法采用白名单，**没有开放 `thread/resume` 或 `turn/start`**。登录只能通过用户点击触发官方登录流程，后台不会切换账号或付费方式。程序不修改 Codex 的会话数据库、日志或配置。

未来开放执行必须先补齐可验证的执行所有权和原配置，再接入持久化发送意图；不能仅删除白名单。`turn/start` 遇到并发任务可能注入当前轮次，`clientUserMessageId` 也不是服务端去重承诺。详见 [协议核查](docs/protocol-audit.md) 和 [产品方案](PLAN.md)。

应用仅保存任务标识、标题、工作目录、状态与自身事件；任务历史仅在内存中读取，不保存正文。数据库位于系统应用数据目录 `io.codex-autoresume.desktop`。移除任务不会删除 Codex 的原会话。

MIT License。独立开源项目，与 OpenAI 无隶属关系。
