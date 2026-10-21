# 原聊天条件续跑：执行端补丁

这是基于 OpenAI Codex **`rust-v0.163.0`** 的修改版源码补丁，**不是 OpenAI 官方发行版**。它补充执行端的条件检查，供 AutoResume 桥接原 Desktop 实例使用，不把旧任务转到终端或新建聊天。

当前是开发集成材料。本机 macOS ARM64 编译与 8 项新增执行端隔离测试已通过，真实 Desktop 与额度恢复尚未验收；不能据此声称已经可用。Windows 激活尚未支持。安装、替换或重启用户的 Codex 客户端不由本脚本执行。

## 构建与测试

需要 Node.js 20+、Git、tar、Rust 1.95.0，以及上游 Codex 的平台编译依赖。macOS 需要 Xcode Command Line Tools；Windows 需要对应的 MSVC 编译环境。依赖与源码体积较大，首次构建需要时间和磁盘空间。

在项目根目录运行：

```sh
# 只校验官方源码、解压并应用补丁，不编译
node scripts/build-guarded-codex.mjs --prepare-only

# 编译修改版独立 App Server
node scripts/build-guarded-codex.mjs

# 编译并运行 sandbox 转发、失败状态、启动参数、mock 续跑、Core 竞态五组测试
node scripts/build-guarded-codex.mjs --test
```

脚本使用 [upstream.json](upstream.json) 固定官方 tag、下载地址和 SHA256。每次从校验后的压缩包解压干净源码，再应用补丁。源码、缓存、构建产物都放在忽略的 `work/guarded-codex-build/`。构建禁用调试信息和增量缓存，最多使用 2 个并发任务。`--test` 使用本地 mock，不登录真实账号，不刻意消耗真实额度。

成功后产物位于 `work/guarded-codex-build/bin/codex-app-server`，Windows 文件名带 `.exe`。构建完成不代表已经接入 Desktop。

### 官方 sandbox 辅助入口

MCP 会通过当前执行端的 `sandbox` 子命令探测及启动官方沙盒。[sandbox-forwarding.patch](sandbox-forwarding.patch) 在原有 arg0 辅助入口分发之后，仅将首参数恰为 `sandbox` 的调用转给 `AUTORESUME_OFFICIAL_CLI` 指定的官方 CLI。该路径由桥接层的已验证配置注入；不使用通用的 `CODEX_CLI_PATH` 推断。普通 App Server、apply_patch 和文件系统辅助入口仍由修改版执行端处理。

Unix 转发使用 `exec`，保留原始参数、环境、工作目录和标准输入输出；官方进程失败后不会重试或启动第二个 App Server。路径缺失、相对路径、不可执行文件或指向自身（含硬链接）时拒绝执行。Windows 的这条转发路径明确拒绝，不能据此声称已支持 Desktop 激活。

`--test` 会先用临时 mock CLI 实际验证参数、退出码/信号、输入输出、环境与递归保护，也可在准备源码后单独运行 `node scripts/test-guarded-sandbox.mjs <准备目录>/codex-rs`。该小测试不编译完整上游、不启动真实 Codex。此补丁不解决语音资源包布局与构建身份兼容，也不伪造官方构建标识。

### 锁文件修复

上游发布归档的 workspace 版本为 `0.163.0`，但归档中 160 个本地包的锁文件版本仍为 `0.0.0`，直接 `--locked` 会失败。[release-lock-versions.patch](release-lock-versions.patch) 仅同步这 160 个本地版本。脚本校验修复前后 SHA256，并逐块确认外部依赖、来源、校验值及依赖列表不变；不运行依赖升级，随后以 `--locked` 构建。

## 新增协议

初始化后查询能力：

```json
{"id":"ar:cap","method":"autoresume/capabilities","params":{}}
```

返回 `{"version":1,"conditionalContinuation":true}`。普通官方服务端不支持这个独立方法，AutoResume 必须拒绝启用执行，不能退回无条件的 `turn/start`。

条件续跑：

```json
{"id":"ar:continue","method":"turn/continueIfFailed","params":{"threadId":"原聊天ID","expectedPreviousTurnId":"额度失败轮次ID"}}
```

成功返回 `{"status":"started","turnId":"新轮次ID"}`；拒绝返回 `{"status":"rejected","reason":"原因"}`。原聊天 ID 不变，正常的轮次事件仍通过原连接送回 Desktop。

仅接受这两个参数，未知字段直接拒绝。不能传入 prompt、模型、审批或沙盒覆盖。服务端使用固定的内部续跑指令，并保留已经核对的原执行配置与非持久单轮选项。

## 安全条件与边界

- 原线程必须已在本进程加载，调用连接已订阅，原 listener 仍有效。
- 本 live generation 必须实际观察到该轮结束，结构化错误恰为 `usageLimitExceeded`；历史日志、自然语言错误、正常完成均不授权执行。
- 必须保存同轮原生 `turn/start` 的实际配置；缺少证据时拒绝。重启后不会自动继承旧失败或旧授权。
- Core 在提交时原子检查空闲状态与上一轮 ID。用户新任务、Goal、队列等已经抢先开始时拒绝，不能把“继续”塞进新任务，也不排队等待以后执行。
- 用户停止、提交/追加输入、配置变化、listener 丢失会使旧资格失效；审批和补充信息仍在原 Desktop 处理。
- 每次失败只能领取一次提交资格。明确拒绝不写入续跑历史；RPC 错误、超时或连接丢失视为结果不明，不自动重试。
- 原生流量透传、账号额度核对、用户主动开启、提交意图持久化和连接生命周期由 AutoResume 桥接层负责。这个补丁本身不会定时提交。

拒绝原因：`missingPreviousTurn`、`notLoaded`、`notSubscribed`、`liveGenerationChanged`、`notIdle`、`noMatchingQuotaFailure`、`configurationUnverified`、`superseded`、`pendingTriggerTurn`、`serverDraining`、`guardRejected`。

## 来源、修改与许可

来源：[OpenAI Codex rust-v0.163.0](https://github.com/openai/codex/tree/rust-v0.163.0)。2026-10-20 至 2026-10-21 的修改集中在协议类型与路由、实时失败证据、条件提交、Desktop 启动参数及 sandbox 辅助入口兼容，以及对应测试；没有修改官方已安装应用。

补丁及其派生运行时遵循随附 [Apache License 2.0](LICENSE)，保留上游 [NOTICE](NOTICE)。AutoResume 主项目的 MIT 许可不替代上游及其依赖的许可。若分发编译产物，需同时保留许可、通知与修改说明，并清楚标明是修改版。
