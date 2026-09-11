# 壳更新通道验证报告（实测，2026-09-11）

> 回答用户问题：**「更新路径通过什么？是通过 GitHub？」**
> 结论：**不能是 GitHub Release 直连**。以下全部为实测 / 源码级证据。

---

## 一、决定性实测：GitHub Release 直连不可用

用**本项目真实产物**测试（壳仓 v0.1.0 的 Release 资产）：

| 目标 | 结果 |
|---|---|
| `objects.githubusercontent.com`（Release 资产 CDN） | **15s 完全超时** |
| GitHub Release 资产下载（3.8MB deb） | **14.8 KB/s**（15.6s 仅下 231KB） |
| GitHub Release 资产下载（3.1MB dmg） | **28 KB/s**（20s 仅下 561KB） |
| `api.github.com`（仅列元数据） | 0.39s（快，但不能下产物） |
| `github.com` 主站 | 8.0s（很慢） |

**推论**：77MB 的 AppImage 在 15–28 KB/s 下需 **约 45 分钟**。
更致命的是——**Tauri updater 的传输超时会先触发，更新永远失败**，而不是只是慢。

### 为什么这恰好印证了内核既有机制的正确性

内核 / DSH 的更新**早就因为这个原因走了 npm 镜像**：

| 通道 | 实测速度 |
|---|---|
| npmmirror（内核在用） | **1.44 MB/s** |
| npm 官方源 | 160 KB/s |

壳的更新通道**不该例外**。

---

## 二、Tauri 源码级验证：url 接受任意 HTTPS URL

`tauri-plugin-updater/src/updater.rs`（v2 分支）原文：

```rust
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ReleaseManifestPlatform {
    /// Download URL for the platform
    pub url: Url,          // url::Url 通用类型，无域名白名单
    pub signature: String,
}
```

下载与校验逻辑：

```rust
verify_signature(&buffer, &self.signature, &self.context.config.pubkey)?;
```

**结论**：
- `url` 字段是**通用 URL**，可指向任意 HTTPS 位置（不限于 GitHub）
- **签名校验独立于托管位置**（minisign + pubkey），换 CDN 不影响安全性
- 源码中**未发现 host 白名单 / allowlist**（仅 `dangerousInsecureTransportProtocol` 控制 HTTP/HTTPS）

---

## 三、npm CDN 可直链包内文件（实测）

`unpkg` / `jsdelivr` 可直接提供 **npm 包内任意单个文件**（**无需**下载整个 tarball）：

| CDN | 小文件（855KB） | **大文件（10MB）** |
|---|---|---|
| **unpkg** | 268 KB/s | **1.71 MB/s** |
| **jsdelivr** | 337 KB/s | 735 KB/s |
| fastly.jsdelivr | 28 KB/s | — |
| gcore.jsdelivr | 216 KB/s | — |
| npmmirror 的 /files/ 路径 | **403**（不提供文件级访问） | — |

**关键洞察**：**小文件时 TLS/延迟开销占比大，大文件时吞吐上来** ——
所以对真实产物（几 MB 至几十 MB），unpkg 达 **1.71 MB/s**，与内核查镜像同级。

### npm 承载大二进制有先例

| 包 | unpackedSize |
|---|---|
| `@napi-rs/canvas-linux-x64-gnu` | **33 MB** |
| `@esbuild/linux-x64` | 11 MB |

说明 npm 完全可承载壳级别（3–77MB）的二进制产物。

---

## 四、重大修正：Tauri 支持 deb/rpm 自更新

**我之前判断「Linux 只有 AppImage 是更新产物、deb 用户出局」是错的。** 源码证据：

```rust
fn install_inner(&self, bytes: &[u8]) -> Result<()> {
    match installer_for_bundle_type(bundle_type()) {
        Some(Installer::Deb) => self.install_deb(bytes),
        Some(Installer::Rpm) => self.install_rpm(bytes),
        _ => self.install_appimage(bytes),
    }
}

fn install_deb(&self, bytes: &[u8]) -> Result<()> {
    if !infer::archive::is_deb(bytes) { return Err(Error::InvalidUpdaterFormat); }
    self.try_tmp_locations(bytes, "dpkg", "-i", "deb")
}

// 提权路径：pkexec（图形 sudo 提示）→ zenity/kdialog 图形密码 → sudo
```

### 体积 / 速度对比（本项目真实产物）

| 平台产物 | 体积 | unpkg @1.71MB/s | 是否需提权 |
|---|---|---|---|
| Linux **deb** | **3.8 MB** | **约 2.2 秒** | 需（pkexec 弹窗） |
| Linux AppImage | **77 MB** | 约 45 秒 | 不需 |
| macOS dmg | 3.1 MB | 约 1.8 秒 | 不需 |
| Windows msi | 3.7 MB | 约 2.2 秒 | 视安装模式 |

**deb 比 AppImage 小 20 倍**。这是之前未被考虑的选项。

> 待实测项：Tauri 是否为 deb 自动生成 `.sig`（官方文档的 v2 产物列表只列了 AppImage/macOS/Windows，未提 deb）。
> 若未自动生成，**我们自己用 minisign 签 deb 即可**（清单里的 `signature` 只需能用 pubkey 验证，与文件来源无关）。
> 此项需 Rust 构建环境（本机当前无 cargo）才能最终确认。

---

## 五、通道方案对比（基于实测）

| 方案 | 速度 | 新增基础设施 | 供应链风险 | 保留官方安装机制 |
|---|---|---|---|---|
| A. GitHub Release 直连 | 15–28 KB/s | 无 | 无 | 是 |
| B. GitHub + 第三方代理（ghfast 659KB/s） | 中 | 无 | **有**（代理可替换产物，虽有验签兜底） | 是 |
| **C. npm CDN（unpkg/jsdelivr）** | **1.71 MB/s** | **无**（复用已有 npm 发布流程） | 无 | 是 |
| D. 自建 CDN / OSS | 快 | **需成本** | 无 | 是 |

### 推荐：C（npm CDN）

理由：
1. **速度最优**（1.71 MB/s，与内核在用镜像同级）
2. **零新增基础设施**——我们**已经在用 npm 发布内核子包**，壳产物复用同一套流程与账号
3. **无第三方代理的供应链风险**（B 的问题）
4. **零成本**（D 的问题）
5. 冷启动可用（壳直连，不依赖内核）

---

## 六、修正后的更新通道架构

```
【发布侧】
  壳产物（AppImage / .app.tar.gz / -setup.exe / deb）
      -> 发布为 npm 包：@dsh-sup/shell-<os>-<arch>@<version>
      -> 清单也作为包内文件：shell-manifest.json

【获取侧】
  壳启动 -> 读清单（unpkg / jsdelivr 直链）
          -> 取 platforms[<os>-<arch>] = { url, signature }
          -> url 指向 unpkg 上的产物文件
          -> tauri-plugin-updater：下载 -> minisign 验签 -> 平台安装

【加速侧（可选）】
  内核（若在运行）预取并缓存到 ~/.dsh/shell/cache/
  -> 壳优先从本机内核取（本地，秒级），失败再直连公网
```

### 与 Tauri 配置的对应关系

```json
{
  "bundle": { "createUpdaterArtifacts": true },
  "plugins": {
    "updater": {
      "pubkey": "<minisign 公钥>",
      "endpoints": [
        "https://unpkg.com/@dsh-sup/shell-<os>-<arch>@latest/shell-manifest.json"
      ]
    }
  }
}
```

- **HTTPS 原生满足** → **不需要** `dangerousInsecureTransportProtocol`（去掉一个安全妥协）
- 清单格式用 Tauri 的**静态 JSON** 语义：`{ version, platforms: { "<os>-<arch>": { url, signature } } }`

---

## 七、对执行方案的修改点

| 章节 | 原内容 | 修正后 |
|---|---|---|
| 第 1 节 通道 | 「公网发布通道（GitHub Release / CDN）」未定 | **定为 npm CDN（unpkg / jsdelivr 直链）** |
| 第 2.5 节 Tauri 配置 | endpoints 未定 | `endpoints = [unpkg 上的 shell-manifest.json]` |
| P4 发布链 | CI 产出更新产物 + 挂 GitHub Release | **产物发布为 npm 包**（复用已有 npm 发布流程）；GitHub Release 仅作人工下载 / 备用 |
| P5 分发形态 | Linux「AppImage 主通道 / deb 由 apt」 | **新增选项：deb 可自更新（3.8MB，pkexec 提权）**；体积差 20 倍，需重新决策 |
| 风险 K3 | 回环 HTTP vs TLS 强制 | 已消除（HTTPS 原生满足） |
| 新增 | — | **K13：npm CDN 可用性**（unpkg / jsdelivr 均为第三方；缓解：多 CDN 回退 + 内核本地缓存） |

---

## 八、需用户决策的新增项

| # | 决策 | 选项 | 我的建议 |
|---|---|---|---|
| **N1** | **更新通道** | A GitHub直连 / B 代理 / **C npm CDN** / D 自建 | **C**（速度最优 + 零基础设施 + 无供应链风险） |
| **N2** | **Linux 更新主形态** | AppImage（77MB，免密码）/ **deb（3.8MB，需一次密码）** | 倾向 **deb**：体积差 20 倍、速度差 20 倍；提权是一次性成本 |
| **N3** | 是否需要内核本地缓存加速 | 需要 / 不需要 | **需要**（热路径从 45 秒降到秒级；且是离线降级路径） |

