# dsh-supervisor-launcher

**DSH supervisor — DeepSeek Harness 的桌面守卫**（桌面壳 + 环境引导器）

给你的 DeepSeek Harness 配一个自带托盘图标的桌面管家：一键装官方 Node LTS，自动拉起自愈守卫，
浏览器/面板随时看状态，局域网设备安全访问，版本更新有日志可查。

> 壳（本仓库）**开源（MIT）**；守卫内核为**闭源构建物**（npm 分发，见下）。

## 功能

- 🚀 **环境引导**：探测系统 Node.js → 缺失/过旧时内嵌引导页一键安装官方最新 LTS（下载 + SHA256 校验 + 一次授权）
- 🛡️ **守卫拉起**：自动定位已安装内核并启动守护进程，控制面板就绪后直达（端口由内核配置 `apiPort` 决定，动态分配）
- 🔄 **生命周期守卫**（内核提供）：进程保活、故障自动重启、崩溃退避、期望状态语义（启动/停止可控）
- 🌐 **局域网安全访问**：`0.0.0.0:3088 → 127.0.0.1:3080` 反向代理，DSH 官方生态同款回环呈现，`lanToken` 可选
- 📊 **运维面板**：实时状态 / 事件时间线 / 配置中心 / 更新日志 / 一键安装 DSH
- 🧊 **托盘常驻**：关窗=隐藏；菜单直发 启动/停止/重启

## 架构

```
systemd user unit → dsh-supervisor（守卫内核，闭源）→ dsh web (127.0.0.1:3080)
                          │
                          └─ Tauri 壳（本仓库）→ 内容区加载「守卫托管的控制面板」
```

壳与内核通过本地 HTTP API 通信（端口由内核 `config.json` 的 `apiPort` 决定）；
壳源码全量开源，可审计、可贡献。

## 仓库结构（本仓自持）

本仓库**独立可构建、可发布**，不依赖内核仓：

```
├── src-tauri/                  壳源码（Rust + 内嵌引导页）
│   ├── bootstrap/              引导页（纯 HTML/CSS/JS，无构建步骤）
│   ├── src/                    main.rs / env.rs / node.rs / core.rs / update.rs
│   ├── tests/                  引导流程回归 + 更新产物验收（用 Tauri 同源依赖）
│   ├── tauri.conf.json         窗口 / 更新通道（npm CDN 静态清单）配置
│   └── tauri.windows.conf.json Windows 平台覆盖（Tauri 平台配置合并）
├── shell-release/              npm 壳包组装 + shell-manifest.json 生成
├── scripts/                    版本提升（bump-shell.sh）/ 版本自洽校验
├── ci/                         glibc 基座门禁（防「只能在新发行版运行」）
├── docs/                       设计 / 审计文档
└── .github/workflows/          四平台构建 + 产物验收 + npm 发布（tag 触发）
```

## 安装

### 内核（闭源，npm 分发）

内核按平台发布为独立 npm 子包（Node launcher 形态，需 Node ≥18）：

```bash
npm i -g @dsh-sup/dsh-core-<platform>-<arch>   # linux-x64 / darwin-arm64 / darwin-x64 / win-x64
dsh-supervisor self-check                      # guardVersion / node / platform 三段自检
```

### 壳（本仓库）

> 正式安装包在 Releases 下载；从源码构建：

```bash
cd src-tauri && cargo build --release
# 产物: target/release/dsh-supervisor-gui（Linux/Win/macOS 矩阵构建见 workflow）
```

## 开发 / 贡献

```bash
cargo build            # 调试构建
./target/debug/dsh-supervisor-gui --node-plan   # 无头冒烟：环境探针 + 官方最新 LTS
```

版本升级与校验（本仓自持）：

```bash
bash scripts/bump-shell.sh <ver>        # 三处互锁同号：Cargo.toml / tauri.conf.json / Cargo.lock
node scripts/verify-shell-versions.js   # 自洽校验
cargo test                              # 含引导流程回归与更新产物验收
```

感兴趣的方向：引导页交互、多语言、Windows 打包、自动化测试。欢迎提 Issue / PR。

## 截图

<!-- 放真实截图：引导页 / 面板概览 / 托盘菜单 / 局域网开关（可参考仓库 screenshots/） -->

- 引导页：环境检测 + 一键安装
- 面板：实时状态卡 + 事件时间线
- 托盘：常驻菜单

## 许可

- **壳（本仓库）**：MIT License（见 [LICENSE](LICENSE)）
- **内核**：UNLICENSED（闭源，保留所有权利；经 npm 平台子包分发）

## 相关

- DeepSeek Harness: <https://github.com/deepseek-ai/DeepSeek-Harness>

Topics: `dsh` `deepseek` `tauri` `rust` `desktop-app` `guardian` `tray` `lan-proxy` `self-update` `nodejs`