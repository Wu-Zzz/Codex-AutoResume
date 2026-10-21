# Codex AutoResume

macOS / Windows 菜单栏与系统托盘工具，让因额度限制中断的 Codex 任务在额度恢复后继续。支持任务状态、简短活动记录、额度显示，以及深色 / 浅色 / 跟随系统主题。

**Alpha：安装包支持本工具新建的终端任务自动续跑；已有 Codex Desktop / CLI 聊天仅监测。原 Desktop 聊天续跑仍为源码实验，未通过原桌面实机验证；真实额度恢复周期也尚未验证。**

![应用界面，使用示例数据](docs/images/app-preview.png)

## 下载

已发布版本：[v0.1.0-alpha.1](https://github.com/Wu-Zzz/Codex-AutoResume/releases/tag/v0.1.0-alpha.1)。`v0.2.0-alpha.1` 正在准备，尚未发布。

| 系统 | 安装包 | 安装方式 |
| --- | --- | --- |
| macOS · Apple Silicon（M 系列） | [下载 DMG](https://github.com/Wu-Zzz/Codex-AutoResume/releases/download/v0.1.0-alpha.1/Codex-AutoResume-v0.1.0-alpha.1-macos-arm64.dmg) | 打开后拖入 Applications |
| macOS · Intel | [下载 DMG](https://github.com/Wu-Zzz/Codex-AutoResume/releases/download/v0.1.0-alpha.1/Codex-AutoResume-v0.1.0-alpha.1-macos-x64.dmg) | 打开后拖入 Applications |
| Windows · x64 | [下载 EXE](https://github.com/Wu-Zzz/Codex-AutoResume/releases/download/v0.1.0-alpha.1/Codex-AutoResume-v0.1.0-alpha.1-windows-x64.exe) | 运行后完成安装 |

安装包由 GitHub Actions 云端构建，[SHA-256 校验文件](https://github.com/Wu-Zzz/Codex-AutoResume/releases/download/v0.1.0-alpha.1/SHA256SUMS.txt)随版本发布。macOS 仅有 ad-hoc 签名，未经 Apple 开发者签名与公证；Windows 未做代码签名，系统可能显示安全提示。

## 使用

1. 先安装官方 [Codex CLI](https://github.com/openai/codex#quickstart)。本工具不附带 CLI，终端集成已核查的版本为 `0.145.0`。
2. 打开 AutoResume，点击 **添加任务 → 新建终端任务**，选择项目文件夹。
3. 勾选 **额度恢复后自动续跑**，再点击 **打开 Codex 终端**。该选项默认关闭；中断后才打开，不会补续跑此前的中断。
4. 在打开的官方终端中完成登录并输入任务。托管环境独立，首次需另行完成官方 **ChatGPT 登录**；不复制已有凭据，当前不支持 API Key 模式。
5. 保持 AutoResume、该终端和电脑运行。关闭主窗口后仍驻留菜单栏 / 托盘；**防休眠尚未实现**。

查看已有聊天：在 **添加任务 → 历史监测** 中搜索，点击 **加入监测**，再开启监测开关。此开关不授权自动执行。顶部的 **当前仅监测** 可点击查看续跑范围。

### 原 Desktop 聊天（源码实验功能）

源码已实现 **连接 Codex → 正常启动接入**：设置一次，首次退出并重开 Codex 后，照常从原图标启动；每个聊天仍需单独开启续跑。当前限定 macOS 默认 zsh 环境及 Codex Desktop 构建 `20770`，使用明确标识的修改版实验执行端。原 Desktop 实机与真实额度恢复尚未验收，Windows 暂不支持此接入。

**已发布安装包未附完整 Desktop 执行组件。** 当前源码会在组件不完整时禁用接入入口；本地 debug 版本可使用 `work/` 中的构建产物准备。开发方法与限制见[实验桌面连接](docs/desktop-bridge.md)。

## 续跑规则与限制

| 情况 | 实际行为 |
| --- | --- |
| 托管任务明确因额度耗尽结束 | 仅接受官方结构化 `usageLimitExceeded`，核对额度与会话状态后在同一终端会话继续 |
| 额度窗口或重置时间未知 | 显示未知；不编造百分比、倒计时，也不固定等待五小时 |
| 一轮正常结束 / 用户主动停止 | 不自动发送“继续” |
| 等待审批 / 需要补充信息 | 停止自动续跑，由用户回官方终端处理 |
| 暂停全部 / 关闭续跑开关 | 阻止后续自动提交，**不停止正在执行的任务**；要停止请操作官方终端 |
| 退出或重启应用 / 托管终端断开 | 自动授权失效，不自动恢复旧任务；需重新新建托管任务 |
| 权限、执行归属或提交结果不明 | 停止自动操作，不放宽权限、不盲目重发 |

额度显示以官方返回的窗口和时间为准，到达重置时间不代表一定能立即续跑。移出列表只停止本工具的管理，不删除原 Codex 会话或项目文件。

尚未提供 Git 分支变动检查、系统通知 / Webhook、自动安装 CLI 或开机启动。

## 验证状态

- **自动检查**：模拟测试与 macOS / Windows CI 覆盖调度、持久化、协议和构建；实验桌面连接使用模拟后端测试。
- **安装检查**：发布流程检查 Mac 镜像及签名、Windows 静默安装，并生成三个安装包的 SHA-256；这些检查不等于实机交互验收。
- **实机验证**：Mac Apple Silicon 已完成一次真实普通任务执行；真实额度耗尽与恢复、原 Desktop 聊天续跑、Windows / Intel Mac 交互均未完成验收。

详细测试范围、连接修正与未验证项见[验证记录](docs/VALIDATION.md)。

## 开发

Tauri 2 + Vue 3 + TypeScript + Rust + SQLite，通过官方 Codex App Server 集成。需要 Node.js 22.12+、Rust stable 和 [Tauri 系统依赖](https://v2.tauri.app/start/prerequisites/)。

```sh
npm ci
npm run tauri dev
```

打包：`npm run tauri build`。仅预览界面：`npm run dev`，访问显示的地址并附加 `?demo=1`；示例模式不调用 Codex。

开发约束见 [AGENTS.md](AGENTS.md)，实现范围见 [PLAN.md](PLAN.md)，安全与协议细节见[协议核查](docs/protocol-audit.md)。

[MIT License](LICENSE)。独立开源项目，与 OpenAI 无隶属关系。
