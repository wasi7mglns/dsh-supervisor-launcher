#!/usr/bin/env node
'use strict';

// Tauri 静态更新清单生成器（2026-09-11）
//
// 汇总各平台 job 的 manifest-entry.json → shell-manifest.json（Tauri 静态清单语义）。
//
// 为什么用静态清单而非动态变量端点：
//   Tauri 的 {{target}}（linux|windows|darwin）与 {{arch}}（x86_64|aarch64）
//   与 npm 包命名（linux|win|darwin、x64|arm64）不同；把变量直接拼进包名会得到
//   不存在的包。静态清单让「URL 构造」只在一处发生，且三平台行为一致。
//
// 用法：
//   node make-manifest.js --ver 0.2.0 --entries <dir-with-manifest-entry-json...> --out dist/npm-shell/shell-manifest.json [--notes "..."]

const fs = require('node:fs');
const path = require('node:path');

function parseArgs(argv) {
  const o = { entries: [] };
  for (let i = 2; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === '--entries') { o.entries.push(argv[i + 1]); i += 1; }
    else if (a.startsWith('--')) { o[a.slice(2)] = argv[i + 1]; i += 1; }
  }
  return o;
}

function walk(dir) {
  const out = [];
  if (!fs.existsSync(dir)) return out;
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) out.push(...walk(p));
    else if (e.name === 'manifest-entry.json') out.push(p);
  }
  return out;
}

function main() {
  const a = parseArgs(process.argv);
  const ver = a.ver;
  if (!ver) { console.error('缺少 --ver'); process.exit(2); }
  const out = a.out || 'dist/npm-shell/shell-manifest.json';

  const files = [];
  for (const d of a.entries) files.push(...walk(d));
  if (!files.length) { console.error('未找到任何 manifest-entry.json'); process.exit(1); }

  const platforms = {};
  const notes = [];
  for (const f of files) {
    const e = JSON.parse(fs.readFileSync(f, 'utf8'));
    if (e.version !== ver) { console.error('版本不一致: ' + e.platform + ' 是 ' + e.version + '，期望 ' + ver); process.exit(1); }
    for (const it of e.entries) {
      if (!it.sig) { console.error('产物缺 .sig: ' + e.platform + '/' + it.name); process.exit(1); }
      platforms[e.manifestKey] = { signature: it.sig, url: a.base || '', name: it.name };
    }
    notes.push(e.platform);
  }

  // URL 前缀：默认按「产物随 npm 包发布」约定构造；可通过 --base 覆盖。
  const baseTpl = a.base || 'https://unpkg.com/@dsh-sup/shell-__PLATFORM__@__VERSION__/artifact/';
  for (const key of Object.keys(platforms)) {
    const p = platforms[key];
    if (!p.url) {
      const plat = Object.keys(platforms).length ? key : key;
      p.url = baseTpl.replace('__PLATFORM__', platToPkg(p.name, key)).replace('__VERSION__', ver) + encodeURIComponent(p.name);
    }
    delete p.name;
  }

  const manifest = {
    version: ver,
    notes: a.notes || ('dsh-supervisor desktop shell ' + ver + ' (' + notes.join(', ') + ')'),
    pub_date: new Date().toISOString(),
    platforms,
  };

  fs.mkdirSync(path.dirname(out), { recursive: true });
  fs.writeFileSync(out, JSON.stringify(manifest, null, 2) + '\n');
  console.log('== 清单已生成: ' + out + ' ==');
  console.log('   版本: ' + ver);
  for (const k of Object.keys(platforms)) console.log('   ' + k + ' -> ' + platforms[k].url);
}

// 由清单键反推 npm 包平台后缀（OS-ARCH → npm 命名）
function platToPkg(_name, key) {
  const MAP = {
    'linux-x86_64': 'linux-x64', 'linux-aarch64': 'linux-arm64',
    'darwin-x86_64': 'darwin-x64', 'darwin-aarch64': 'darwin-arm64',
    'windows-x86_64': 'win-x64', 'windows-aarch64': 'win-arm64',
  };
  return MAP[key] || key;
}

main();
