# LLM Meter

[English](README.md) | **简体中文**

[![llm-meter-cli on crates.io](https://img.shields.io/crates/v/llm-meter-cli.svg?label=llm-meter-cli)](https://crates.io/crates/llm-meter-cli)
[![llm-meter-daemon on crates.io](https://img.shields.io/crates/v/llm-meter-daemon.svg?label=llm-meter-daemon)](https://crates.io/crates/llm-meter-daemon)
[![crates.io downloads](https://img.shields.io/crates/d/llm-meter-cli.svg?label=downloads)](https://crates.io/crates/llm-meter-cli)
[![license](https://img.shields.io/crates/l/llm-meter-cli.svg)](https://crates.io/crates/llm-meter-cli)

LLM Meter 是一个本地优先（local-first）的 LLM 用量、成本与配额监控工具，支持
Linux 和 Windows。它由 Rust 守护进程、CLI、原生 Linux 顶栏集成，以及 Tauri
桌面端与托盘应用组成。

当前的 provider 实现支持 OpenAI Platform 账号、ChatGPT 订阅，以及
Kimi（月之暗面）Code 订阅。LLM Meter 还能发现本地正在运行的 Codex 会话，
按模型拆分其 Token 用量，并计算 API 等价成本估算。

## 版本 0.2.0

- 新增对 New API、OpenRouter 以及通用 OpenAI 兼容端点的 relay provider 支持。
- 新增本地带认证的 relay 代理，支持按客户端凭据与用量计费。
- 在 CLI、桌面端 UI 和原生 Noctalia 面板中扩展 relay 管理能力。
- 修复 Noctalia 顶栏组件宽度协商问题，七日 Token 趋势不再被截断。

## 功能特性

- ChatGPT 订阅配额、每周重置时间，以及重置额度的过期时间。
- 通过 OAuth 设备码登录获取 Kimi（月之暗面）Code 订阅配额与用量。
- 内置 OpenAI、Kimi、MiniMax、GLM、MiMo、Claude、DeepSeek 和 Gemini 的价目表。
- 基于近期本地活动估算配额耗尽时间。
- 通过受支持的 API 凭据采集 OpenAI Platform 用量与成本。
- 采集 New API 和 OpenRouter 用量，并支持通用 OpenAI 兼容 relay。
- 带认证的本地 relay 代理，支持按客户端凭据与用量计费。
- 每两秒自动发现正在运行的 Codex 会话。
- 按模型统计输入、缓存输入、输出和总 Token。
- 本地自然日的 Token 与 API 等价成本累计，在 Codex 退出和守护进程重启后仍然保留。
- 原生 Noctalia 弹窗，含总览、连接、登录、预算、重置额度和本地 Codex 页面。
- 可配置的顶栏字段：账号、配额、今日 Token、今日成本、活跃 Codex 数量和七日趋势。
- Waybar JSON 输出、CLI 诊断、SQLite 历史、告警和预算。
- 密钥存放在操作系统凭据库中，而非 SQLite。

> [!IMPORTANT]
> ChatGPT 订阅并非按 Token 计费。LLM Meter 显示的本地 Codex 成本是使用等价
> API 文本 Token 价格得出的**估算值**，不是额外收费，也不是账单。内置价目表目前
> 覆盖 OpenAI、Kimi、MiniMax、GLM、MiMo、Claude、DeepSeek 和 Gemini；价格可能
> 变动，每个 provider 都会记录其来源和生效日期。

## 架构

```text
OpenAI / Codex 日志
        │
        ▼
   llm-meterd ─── SQLite + 操作系统凭据库
        │ 私有本地 IPC
        ├──────── llm-meter CLI
        ├──────── Noctalia / Waybar
        └──────── Tauri 桌面端
```

`llm-meterd` 负责 provider 访问、数据规范化、本地 Codex 采集、存储和告警。
各前端通过私有本地 IPC 消费经白名单过滤的快照：Linux 上是 Unix socket，
Windows 上是每用户命名管道。密钥输入可能会短暂经过这条受保护的通道，但绝不会
保留在常规快照 DTO、SQLite、配置、日志或持久化的 WebView 状态中。

## 环境要求

- 从源码构建需要支持 edition 2024 的 Rust stable。
- Linux 需要一个具备 `XDG_RUNTIME_DIR` 的用户会话，以及一个 Secret Service
  实现，例如 GNOME Keyring 或 KeePassXC。Noctalia/Quickshell 和 Waybar 是可选的
  Linux 集成；Hyprland 直接受支持。
- Windows 需要 Windows 10 22H2 或仍受维护的 Windows 11 版本。密钥使用 Windows
  凭据管理器，桌面端使用 Microsoft Edge WebView2。NSIS 安装包会在需要时自动配置
  WebView2 引导程序。

在 Linux 上构建桌面端还需要常规的 Tauri v2 Linux 依赖，包括 WebKitGTK 4.1 和
JavaScriptCoreGTK 4.1。

## 安装

### Windows 安装包

从对应的 GitHub Release 下载
`llm-meter-vX.Y.Z-x86_64-pc-windows-msvc-unsigned-preview-setup.exe`，以当前用户
身份运行。安装包包含 Tauri 桌面端、CLI 和 `llm-meterd.exe` sidecar，并注册守护
进程的每用户计划任务。同时提供便携版 ZIP，用于测试和恢复；它仍将应用数据存放在
`%LOCALAPPDATA%` 下，密钥存放在 Windows 凭据管理器中。

当前自动构建的 Windows 产物是未签名预览版。其文件名包含 `unsigned-preview`，
Windows 可能提示未知发布者。它们不是生产签名版本；发布的 SHA-256 校验和可验证
文件完整性，但不代表发布者身份。

桌面端为单实例。正常启动会显示可拖拽的紧凑组件，它是 Noctalia `BarWidget` 在
Windows 上的对应物。点击组件可切换 420 x 480 的状态面板，拖动可把它放到顺手的
位置，右键可打开主窗口。其位置在多次启动之间会被记住。当配置了桌面登录自启时，
`--hide` 只会创建托盘。守护进程通过 `llm-meter autostart` 配置的每用户计划任务
运行，桌面端也能在有限等待时间内恢复离线的同级守护进程。退出桌面托盘不会隐式
删除数据或凭据。

受支持的系统、路径、打包、安全与故障排查请参见
[Windows 集成基线](docs/windows-integration-v0.1.md)。
在 Windows 上，`llm-meter update` 会打开官方最新 Release；请安装新的当前用户
NSIS 包，而不要通过 Cargo 替换正在运行的可执行文件。

### 使用 Cargo 安装 Linux 稳定版

日常使用请从 crates.io 安装最新的稳定发布版。这样可以避免跟踪仓库 `main` 分支上
未完成的变更。如果还没有 `cargo`，请先用 [rustup](https://rustup.rs/) 安装 Rust
stable 工具链：

```bash
rustup toolchain install stable
rustup default stable
```

然后使用其发布的 lockfile 安装应用包：

```bash
cargo install --locked llm-meter-cli
llm-meter setup
```

包页面：

- [`llm-meter-cli`](https://crates.io/crates/llm-meter-cli) 会同时安装
  `llm-meter` 和 `llm-meterd`，并包含部署向导；
- [`llm-meter-daemon`](https://crates.io/crates/llm-meter-daemon) 是可复用的守护
  进程运行时库，会作为依赖自动引入，无需单独安装。

Cargo 默认将可执行文件安装到 `~/.cargo/bin`。如果 shell 找不到 `llm-meter`，
请把该目录加入 `PATH` 并重开 shell：

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

如需安装指定稳定版本而非最新版：

```bash
cargo install --locked --version 0.2.0 llm-meter-cli
```

升级已有的 Cargo 安装：

```bash
llm-meter update
```

更新器会安装 crates.io 上最新的稳定版，刷新内置的桌面集成，并且仅在守护进程原本
就在运行时才重启它。对于尚未提供 `update` 的旧版本，先手动执行一次
`cargo install --locked --force llm-meter-cli`。

Cargo 有意不运行包的安装后钩子，因此需要显式执行 `setup` 步骤，它会在修改桌面
配置前安全地征求确认。向导会分别提供桌面环境和顶栏的选择。当前发布版支持
**Hyprland + Noctalia**；后续可以把更多集成加入同一个选择器。

无人值守部署时，请显式指定选项：

```bash
llm-meter setup --desktop hyprland --bar noctalia --non-interactive
```

使用 `--dry-run` 可在不写入文件的情况下查看计划。`setup` 命令会安装内置的
Noctalia 插件、添加 Hyprland 规则、尽可能重载受支持的组件，并安装 `llm-meterd`
systemd 用户服务。其用户会话自启状态可在 Noctalia 设置面板或命令行中修改：

```bash
llm-meter autostart status
llm-meter autostart enable
llm-meter autostart disable
```

可选的 Tauri 桌面端二进制也会附在带版本号的 GitHub Release 中；对于守护进程、
CLI 和原生 Noctalia 体验来说，Cargo 安装已经足够。

### 从源码安装开发版

克隆 `main` 会安装最新的开发状态，可能包含未完成或破坏性变更。此路径适用于贡献
代码、测试未发布的修复，或开发桌面端应用：

```bash
git clone https://github.com/huihui110120/LLM-Meter.git
cd llm-meter
make install
```

日常前台开发流程请运行 `make run`。

`make run` 会构建工作区和桌面端应用，把开发二进制安装到 `~/.local/bin`，检测当前
受支持的顶栏，同步其集成，仅在必要时重载 LLM Meter 的 Noctalia 插件，并在前台
运行开发版守护进程。

常用开发命令：

```bash
make run              # 构建、同步桌面/顶栏集成、运行守护进程
make daemon           # 只构建并运行 llm-meterd
make popup            # 打开紧凑弹窗应用
make gui              # 打开完整桌面端应用
make sync-noctalia    # 安装/热重载原生 Noctalia 插件
make sync-waybar      # 安装/重载 Waybar 集成
make check
make test
```

## 账号与认证

原生弹窗允许你在登录前先选择 provider。它支持基于浏览器的 ChatGPT（OpenAI）和
Kimi（月之暗面）登录，并管理生成的连接。等效的 CLI 命令如下：

```bash
# OpenAI ChatGPT（浏览器登录，默认）
llm-meter add subscription --provider openai --open --name "OpenAI ChatGPT"
llm-meter add subscription --provider openai --device

# Kimi（月之暗面）设备码登录
llm-meter add subscription --provider kimi --open --name Kimi

# OpenAI API key
llm-meter add admin
llm-meter add standard
```

- `subscription` 同时支持 OpenAI 和 Kimi 账号流程；用 `--provider` 选择，用
  `--open` 打开浏览器完成 OAuth/设备码步骤。
- `admin` 请求 OpenAI Admin API key。
- `standard` 请求标准 OpenAI API key。
- 自动化场景下，把一行 API key 通过管道传给
  `llm-meter add admin --secret-stdin` 或 `llm-meter add standard --secret-stdin`；
  API key 绝不接受通过命令行参数传入。

API key 和 OAuth 密钥会存入系统 Keyring。SQLite 数据库只保存凭据引用和非密钥的
账号状态。ChatGPT 订阅适配器可以复用本地已安装的 Codex 应用所持有的认证信息，
而不必重复登录。

## CLI

```bash
llm-meter status
llm-meter connections
llm-meter diagnostics
llm-meter refresh CONNECTION_ID
llm-meter refresh-all
llm-meter remove CONNECTION_ID
llm-meter budget CONNECTION_ID 20 --currency USD
llm-meter waybar
llm-meter waybar --watch
```

`refresh-all` 会等待所有已配置的 provider 连接完成一次真实网络刷新，并立即刷新
本地 Codex 采集器。provider 自动刷新默认为：ChatGPT 和 Kimi 订阅五分钟，
Platform 用量十分钟。本地 Codex 发现每两秒运行一次。

## Noctalia 与顶栏设置

点击 LLM Meter 药丸图标即可打开原生半透明弹窗。设置、登录、预算和顶栏自定义都
无需打开完整的 Tauri GUI。

在 **设置 → 顶栏显示** 下，以下各项均可独立启用或禁用：

- 账号名称；
- 每周剩余配额；
- 今日本地 Codex Token；
- 今日 API 等价成本；
- 活跃 Codex 会话数；
- 七日 Token 趋势。

设置由 Noctalia 持久化，并立即更新顶栏。源同步使用仅插件级热重载并带 QML 缓存
清除参数；当文件未变化或插件重载已足够时，不会重启 Noctalia 本身。

## 本地 Codex 计量

Codex 会话日志从以下位置增量读取：

```text
~/.codex/sessions/YYYY/MM/DD/*.jsonl
```

在 Linux 上，LLM Meter 通过 `/proc` 检测已打开的会话文件；Windows 使用其平台
采集器。两种实现都会记录模型切换，并把累计 Token 的增量归给当时活跃的模型。
LLM Meter 还会读取当前及相邻的 UTC 日期目录，因此 Codex 进程退出后今日本地累计
值不会消失。新的本地自然日按机器时区从零开始。

原始 Codex JSONL 文件仍归 Codex 所有；LLM Meter 不会把它们复制进自己的数据库。
因此，本地日窗口之外已关闭的历史会话不会作为 LLM Meter 的全量账本呈现。

## 文件与数据

持久化状态组织在 `~/.llm-meter` 下：

```text
~/.llm-meter/
├── config.toml
├── data/
│   ├── llm-meter.sqlite3
│   ├── llm-meter.sqlite3-wal
│   └── llm-meter.sqlite3-shm
└── logs/
    └── llm-meterd.log
```

设置 `LLM_METER_HOME` 可改用其他持久化根目录。首次启动时，若新目标为空，守护
进程会迁移早期的 XDG 数据/配置布局。

在 Linux 上，以下位置有意保持独立：

- 运行时 socket：`$XDG_RUNTIME_DIR/llm-meter/daemon.sock`；
- 敏感凭据：操作系统 Keyring/Secret Service；
- Codex 拥有的会话与认证状态：`~/.codex`；
- Noctalia 插件安装位置：`~/.config/noctalia/plugins/llm-meter`。

Windows 将配置、SQLite 数据和日志存放在 `%LOCALAPPDATA%\LLM Meter` 下；凭据保留
在 Windows 凭据管理器中。Windows 命名管道在该目录中没有对应文件。

在 Linux 上，目录以 `0700` 模式创建；数据库、配置和守护进程日志文件使用 `0600`
模式。Windows 依赖当前用户的配置文件 ACL，而非 Unix 权限位。

## 配置

守护进程会以安全默认值创建 `~/.llm-meter/config.toml`：

```toml
scheduler_tick_seconds = 30
subscription_sync_seconds = 300
platform_usage_sync_seconds = 600
manual_refresh_min_seconds = 30
stale_after_seconds = 1800

[retention]
raw_days = 30
hourly_days = 180
provider_events_days = 30
```

参见 [`config.example.toml`](https://github.com/huihui110120/LLM-Meter/blob/main/config.example.toml)。
配置文件中不得包含凭据。

## systemd 与 Hyprland（Linux）

打包模板位于 `packaging/` 下：

- `packaging/systemd/llm-meterd.service`；
- `packaging/systemd/llm-meter-desktop.service`；
- `packaging/hyprland/llm-meter.lua` 和 `.conf`；
- `packaging/noctalia/llm-meter/`；
- `packaging/waybar/`。

安装守护进程 unit 后：

```bash
systemctl --user daemon-reload
systemctl --user enable --now llm-meterd.service
journalctl --user -u llm-meterd.service -f
```

手动打包的详细说明请参见
[Hyprland 集成指南](https://github.com/huihui110120/LLM-Meter/blob/main/packaging/README-hyprland.md)。

## 开发

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo build --manifest-path apps/desktop/src-tauri/Cargo.toml
```

Rust 工作区包含：

| 包                                                                                  | 用途                                          |
| ----------------------------------------------------------------------------------- | --------------------------------------------- |
| [`llm-meter-core`](https://crates.io/crates/llm-meter-core)                         | 与 provider 无关的领域类型和适配器契约         |
| [`llm-meter-storage`](https://crates.io/crates/llm-meter-storage)                   | SQLite 仓储、迁移与保留策略                    |
| [`llm-meter-secret-store`](https://crates.io/crates/llm-meter-secret-store)         | 基于原生 Keyring 的密钥存储                    |
| [`llm-meter-provider-openai`](https://crates.io/crates/llm-meter-provider-openai)   | OpenAI 订阅与 Platform 适配器                  |
| [`llm-meter-provider-testkit`](https://crates.io/crates/llm-meter-provider-testkit) | Mock provider 与适配器测试辅助工具             |
| [`llm-meter-daemon`](https://crates.io/crates/llm-meter-daemon)                     | 可复用的调度器、采集器与 IPC 运行时            |
| [`llm-meter-cli`](https://crates.io/crates/llm-meter-cli)                           | `llm-meter` 和 `llm-meterd` 可执行文件包       |

Tauri 桌面端包有意排除在 crates.io 发布之外。

## 发布 crate

发布必须遵循依赖顺序：

1. `llm-meter-core`；
2. `llm-meter-secret-store`、`llm-meter-storage`、
   `llm-meter-provider-openai`、`llm-meter-provider-kimi` 和
   `llm-meter-relay-proxy`；
3. `llm-meter-provider-relay` 和 `llm-meter-provider-testkit`；
4. `llm-meter-daemon`；
5. `llm-meter-cli`。

每发布一个新版本，都要等它出现在 registry 索引中后再继续。虽然
`llm-meter-provider-testkit` 只是开发辅助工具，但守护进程将其声明为带版本号的
开发依赖，因此要在守护进程之前发布它。

发布前，对每个包运行
`cargo publish --dry-run --registry crates-io -p PACKAGE --allow-dirty`。当本地
Cargo 配置用镜像替换了默认 crates.io 索引时，显式指定 registry 同样有效。下游的
dry-run 只有在所选 registry 中存在其确切版本后才能解析内部 crate；首次正式发布
前，可用 `cargo package --no-verify` 检查其归档内容而不上传。

## GitHub Release

推送与所有 Cargo 包版本一致的语义化版本 tag，会自动创建 GitHub Release：

```bash
git tag v0.2.0
git push origin v0.2.0
```

发布工作流会运行工作区测试，构建 CLI、守护进程和 Tauri 桌面端二进制，并发布带
版本号的 Linux 归档、明确标注为未签名预览版的 Windows NSIS 安装包与便携版 ZIP，
以及合并的 `SHA256SUMS`。它不会向 crates.io 发布 crate；crate 发布仍是上文所述
的独立、有序流程。

## 文档

详细的架构与桌面集成文档索引在
[`docs/README.md`](https://github.com/huihui110120/LLM-Meter/blob/main/docs/README.md)。

## 许可证

LLM Meter 基于 MIT 许可证授权。参见 [`LICENSE`](LICENSE)。
