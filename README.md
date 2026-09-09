# dsh-supervisor-launcher

**DSH supervisor — DeepSeek Harness 的桌面守卫**（桌面壳 + 环境引导器）

给你的 DeepSeek Harness 配一个自带托盘图标的桌面管家：一键装官方 Node LTS，自动拉起自愈守卫，
浏览器/面板随时看状态，局域网设备安全访问，版本更新有日志可查。

> 壳（本仓库）**开源（MIT）**；守卫内核为**闭源 SEA 构建物**（npm 分发，见下）。

## 功能

- 🚀 **环境引导**：探测系统 Node.js → 缺失/过旧时内嵌引导页一键安装官方最新 LTS（下载 + SHA256 校验 + 一次授权）
- 🛡️ **守卫拉起**：自动定位已安装内核并启动守护进程，面板就绪后直达控制台（127.0.0.1:3100）
- 🔄 **生命周期守卫**（内核提供）：进程保活、故障自动重启、崩溃退避、期望状态语义（启动/停止可控）
- 🌐 **局域网安全访问**：`0.0.0.0:3088 → 127.0.0.1:3080` 反向代理，DSH 官方生态同款回环呈现，`lanToken` 可选
- 📊 **运维面板**：实时状态 / 事件时间线 / 配置中心 / 更新日志 / 一键安装 DSH
- 🧊 **托盘常驻**：关窗=隐藏；菜单直发 启动/停止/重启

## 架构

```
systemd user unit → dsh-supervisor（SEA 内核，闭源）→ dsh web (127.0.0.1:3080)
                          │
                          └─ Tauri 壳（本仓库）→ 面板 127.0.0.1:3100
```

壳与内核通过本地 HTTP API（3100）通信，协议公开；壳源码全量开源，可审计、可贡献。

## 安装

### 内核（闭源 SEA 二进制，npm 分发）

```bash
npm i -g @dsh-core/dsh-core-<platform>-<arch>   # linux-x64 / darwin-arm64 / darwin-x64 / win-x64
dsh-supervisor self-check                        # guardVersion / node / platform 三段自检
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

感兴趣的方向：引导页交互、多语言、Windows 打包、自动化测试。欢迎提 Issue / PR。

## 截图

<!-- 放真实截图：引导页 / 面板概览 / 托盘菜单 / 局域网开关（可参考仓库 screenshots/） -->

- 引导页：环境检测 + 一键安装
- 面板：实时状态卡 + 事件时间线
- 托盘：常驻菜单

## 许可

- **壳（本仓库）**：MIT License（见 [LICENSE](LICENSE)）
- **内核**：UNLICENSED（闭源，保留所有权利；SEA 字节码构建物，经 npm 分发）

## 相关

- DeepSeek Harness: <https://github.com/deepseek-ai/DeepSeek-Harness>

Topics: `dsh` `deepseek` `tauri` `rust` `desktop-app` `guardian` `tray` `lan-proxy` `self-update` `nodejs`