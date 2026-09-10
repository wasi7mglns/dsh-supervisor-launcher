#!/usr/bin/env node
'use strict';

// 壳发布产物组装器（2026-09-11 新增，跨平台共用）
//
// 职责：把一个平台矩阵 job 产出的「安装包 + .sig」组装为可发布的 npm 包，
//       并输出供汇总阶段生成 Tauri 静态清单的中间文件。
//
// 为什么需要它：
//   1) Tauri 的 {{target}}/{{arch}} 取值（linux|windows|darwin / x86_64|aarch64）
//      与 npm 包命名（linux|win|darwin / x64|arm64）不同，直接拼进包名会得到不存在的包；
//      故采用【静态清单】解耦：清单内部用 Tauri 的 OS-ARCH 键映射到真实 npm 产物 URL。
//   2) .sig 与安装包必须成对收集，缺 .sig 则自动更新不可用，此处强校验。
//
// 用法：
//   node assemble-shell-pkg.js --platform linux-x64 --ver 0.2.0 \
//        --bundle-dir src-tauri/target/release/bundle --out dist/npm-shell

const fs = require('node:fs');
const path = require('node:path');

// npm 包后缀 → Tauri 清单键（OS-ARCH）+ 运行时安装形态
const PLATFORMS = {
  'linux-x64':    { os: 'linux',  cpu: 'x64',   manifestKey: 'linux-x86_64',   installer: 'deb'  },
  'linux-arm64':  { os: 'linux',  cpu: 'arm64', manifestKey: 'linux-aarch64',  installer: 'deb'  },
  'darwin-x64':   { os: 'darwin', cpu: 'x64',   manifestKey: 'darwin-x86_64',  installer: 'app'  },
  'darwin-arm64': { os: 'darwin', cpu: 'arm64', manifestKey: 'darwin-aarch64', installer: 'app'  },
  'win-x64':      { os: 'win32',  cpu: 'x64',   manifestKey: 'windows-x86_64', installer: 'nsis' },
  'win-arm64':    { os: 'win32',  cpu: 'arm64', manifestKey: 'windows-aarch64',installer: 'nsis' },
};

const ARTIFACT_PATTERNS = {
  deb:  [/\.deb$/],
  app:  [/\.app\.tar\.gz$/],
  nsis: [/-setup\.exe$/i, /\.exe$/i],
};

function parseArgs(argv) {
  const o = {};
  for (let i = 2; i < argv.length; i += 1) {
    const a = argv[i];
    if (a.startsWith('--')) { o[a.slice(2)] = argv[i + 1]; i += 1; }
  }
  return o;
}

function walk(dir) {
  const out = [];
  if (!fs.existsSync(dir)) return out;
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) out.push(...walk(p)); else out.push(p);
  }
  return out;
}

function findArtifacts(bundleDir, installer) {
  const pats = ARTIFACT_PATTERNS[installer] || [];
  const all = walk(bundleDir).filter((f) => !/\.sig$/.test(f));
  const hits = all.filter((f) => pats.some((re) => re.test(path.basename(f))));
  const withSig = hits.filter((f) => fs.existsSync(f + '.sig'));
  return (withSig.length ? withSig : hits).map((f) => ({ file: f, sig: f + '.sig' }));
}

function main() {
  const a = parseArgs(process.argv);
  const plat = a.platform;
  const ver = a.ver;
  const bundleDir = a['bundle-dir'];
  const out = a.out || 'dist/npm-shell';
  const meta = PLATFORMS[plat];
  if (!meta) { console.error('未知平台: ' + plat + '（可用: ' + Object.keys(PLATFORMS).join(', ') + '）'); process.exit(2); }
  if (!ver) { console.error('缺少 --ver'); process.exit(2); }
  if (!bundleDir) { console.error('缺少 --bundle-dir'); process.exit(2); }

  const pkgName = '@dsh-sup/shell-' + plat;
  const stage = path.join(out, pkgName);
  fs.rmSync(stage, { recursive: true, force: true });
  fs.mkdirSync(path.join(stage, 'artifact'), { recursive: true });

  const arts = findArtifacts(bundleDir, meta.installer);
  if (!arts.length) { console.error('未在 ' + bundleDir + ' 找到 ' + meta.installer + ' 产物'); process.exit(1); }

  const entries = [];
  for (const it of arts) {
    const dst = path.join(stage, 'artifact', path.basename(it.file));
    fs.copyFileSync(it.file, dst);
    let sigText = null;
    if (fs.existsSync(it.sig)) {
      sigText = fs.readFileSync(it.sig, 'utf8').trim();
      fs.copyFileSync(it.sig, dst + '.sig');
    }
    entries.push({ name: path.basename(it.file), sig: sigText });
  }

  fs.writeFileSync(path.join(stage, 'package.json'), JSON.stringify({
    name: pkgName, version: ver,
    description: 'DSH supervisor desktop shell updater artifact for ' + plat + ' (Tauri updater).',
    license: 'MIT', os: [meta.os], cpu: [meta.cpu], files: ['artifact'],
  }, null, 2) + '\n');

  fs.writeFileSync(path.join(stage, 'artifact', 'manifest-entry.json'), JSON.stringify({
    platform: plat, manifestKey: meta.manifestKey, version: ver, installer: meta.installer, entries,
  }, null, 2) + '\n');

  console.log('== 组装完成: ' + stage + ' ==');
  console.log('   包名: ' + pkgName + '@' + ver);
  console.log('   清单键: ' + meta.manifestKey);
  for (const e of entries) console.log('   产物: ' + e.name + (e.sig ? ' (+sig)' : ' (无 sig!)'));

  const missing = entries.filter((e) => !e.sig);
  if (missing.length) {
    console.error('❌ 有产物缺 .sig —— 自动更新不可用（请确认 TAURI_SIGNING_PRIVATE_KEY(_PASSWORD) 已配置）');
    process.exit(1);
  }
}

main();
