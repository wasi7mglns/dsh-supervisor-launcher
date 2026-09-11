#!/usr/bin/env bash
# 桌面壳版本提升（**壳仓自持**，2026-09-11 双仓隔离）。
#
# 为什么放在壳仓：壳的版本号与三处互锁全部属于壳自身；此前该逻辑位于内核仓的
# release/scripts/bump.sh（--shell 分支），属「壳资产放在内核仓」的违规之一。
#
# 用法（壳仓根执行）:
#   bash scripts/bump-shell.sh 1.0.4
#
# 只允许递增。三处同步：Cargo.toml / tauri.conf.json / Cargo.lock。
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
NEW="${1:?用法: bash scripts/bump-shell.sh <version>}"
[[ "$NEW" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-(BETA|RC)\.[0-9]+)?$ ]] || { echo "非法版本号: $NEW"; exit 1; }
# SemVer 逐段比较（字符串比较在 0.10 vs 0.2 场景会失效）
ver_lt() {
  node -e "const [a,b]=process.argv.slice(1);const p=(v)=>{const[m,t]=v.split('-');const c=m.split('.').map(Number);const tier=t?(t.startsWith('BETA')?0:1):2;return[c[0],c[1],c[2],tier,t?(Number(t.split('.')[1])||0):0];};const A=p(a),B=p(b);for(let i=0;i<5;i++){if(A[i]<B[i])process.exit(0);if(A[i]>B[i])process.exit(1);}process.exit(1);" "$1" "$2"
}
CUR="$(node -p "require('./src-tauri/tauri.conf.json').version")"
ver_lt "$NEW" "$CUR" && { echo "拒绝回退：$NEW < 当前壳 $CUR"; exit 1; }
sed -i -E "s/^version = .*/version = \"$NEW\"/" src-tauri/Cargo.toml
# Cargo.lock 中本包版本也需同步（否则 cargo 视为依赖变更）
sed -i -E "/^name = \"dsh-supervisor-gui\"$/{n;s/^version = .*/version = \"$NEW\"/}" src-tauri/Cargo.lock
NEW="$NEW" node -e "const fs=require('fs');const p='src-tauri/tauri.conf.json';const j=JSON.parse(fs.readFileSync(p));j.version=process.env.NEW;fs.writeFileSync(p,JSON.stringify(j,null,2)+'\n')"
node scripts/verify-shell-versions.js
echo "=== 壳版本已提升: $CUR → $NEW ==="
echo "  1) 更新 CHANGELOG.md"
echo "  2) git add -A && git commit && git tag v$NEW && git push origin main && git push origin v$NEW"
echo "  公开仓 tag 触发 launcher-build.yml → 四平台 bundle + npm 壳包"
