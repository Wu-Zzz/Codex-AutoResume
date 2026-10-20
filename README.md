# Codex AutoResume

为本地 Codex 任务等待额度恢复的轻量桌面助手。面向 macOS 和 Windows，Tauri 2 + Vue 3 + Rust + SQLite。

**当前是开发预览，不是已经可用的自动接管工具。** 本地会话发现、只读监测、任务管理、托盘和调度核心已经实现。当前公开协议无法证明另一个客户端已停止执行，也无法在恢复前完整读取原权限配置，因此本版本**不会自动发送“继续”**。不能用它无人值守接管正在运行的 Desktop 或 CLI 任务。

![任务列表界面，使用隔离的示例数据](docs/images/app-preview.png)

## 当前功能

- 发现本机官方 Codex，读取 ChatGPT 登录状态、会话列表和真实额度。
- 选择任务、保存自动续跑意愿、暂停全部、移除本工具的监测记录、查看状态与简短记录。
- 关闭窗口后留在菜单栏或系统托盘；退出菜单会明确停止后台监测；重复打开只激活现有窗口。
- SQLite 保存任务、暂停状态、每个失败轮次唯一的发送意图。调度核心验证额度、权限、接管状态和重启后的提交结果。
- 请求超时、未知状态、审批、用户停止、正常结束及不明确的错误不会触发自动续跑。

开关打开时，当前版本会记录选择并监测；接管未通过时显示“需要处理”。真正执行适配器尚未开放，不存在隐藏的“忽略保护”选项。浏览器演示数据与真实账号完全隔离。

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

macOS / Windows 的构建检查已配置在 GitHub Actions；本地写入工作流不代表云端已运行。签名、公证、一键安装 Codex、通知、自启动与保持唤醒尚未实现。Windows 实机与真实跨额度窗口验证单独记录在 [验证记录](docs/VALIDATION.md)。

## 实现边界

Vue 只调用窄接口，Rust 是状态的唯一来源。`protocol.rs` 对 RPC 方法采用白名单，**没有开放 `thread/resume` 或 `turn/start`**。登录只能通过用户点击触发官方登录流程，后台不会切换账号或付费方式。程序不修改 Codex 的会话数据库、日志或配置。

未来开放执行必须先补齐可验证的执行所有权和原配置，再接入持久化发送意图；不能仅删除白名单。`turn/start` 遇到并发任务可能注入当前轮次，`clientUserMessageId` 也不是服务端去重承诺。详见 [协议核查](docs/protocol-audit.md) 和 [产品方案](PLAN.md)。

应用仅保存任务标识、标题、工作目录、状态与自身事件；任务历史仅在内存中读取，不保存正文。数据库位于系统应用数据目录 `io.codex-autoresume.desktop`。移除任务不会删除 Codex 的原会话。

MIT License。独立开源项目，与 OpenAI 无隶属关系。
