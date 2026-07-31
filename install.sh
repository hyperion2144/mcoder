#!/bin/bash
# mcoder 一键构建安装脚本
# 安装到 ~/.cargo/bin/（需 Rust + Bun）
# 用法: ./install.sh
set -e

ROOT="$(cd "$(dirname "$0")" && pwd)"

echo "=== mcoder installer ==="

# 检查依赖
command -v cargo >/dev/null || { echo "ERROR: cargo not found. Install Rust: https://rustup.rs"; exit 1; }
command -v bun  >/dev/null || { echo "ERROR: bun not found. Install: curl -fsSL https://bun.sh/install | bash"; exit 1; }
command -v npm  >/dev/null || { echo "ERROR: npm not found. Install Node.js first."; exit 1; }

# 1. 构建 mcoder (Rust)
echo ""
echo "[1/3] Building mcoder (Rust)..."
cd "$ROOT/mcoder"
cargo install --path . --force

# 2. 构建 mcoder-tui (Bun compile 单文件)
echo ""
echo "[2/3] Building mcoder-tui (Bun standalone)..."
cd "$ROOT/mcoder-tui"
# 删除 lockfile 重新生成，避免 package.json/lockfile 不一致导致 peer dep 冲突
# （历史上 react-devtools-core 在 package.json 写 ^7 但 lockfile 锁 4，npm install 时升级会与 React 18 冲突）
[ -f package-lock.json ] && rm -f package-lock.json
[ -d node_modules ] && rm -rf node_modules
npm install --legacy-peer-deps --silent
npm run build
./build-standalone.sh

# 3. 复制到同目录
echo ""
echo "[3/3] Installing..."
BINDIR="$(cargo home 2>/dev/null || echo "$HOME/.cargo")/bin"
cp "$ROOT/mcoder-tui/mcoder-tui" "$BINDIR/mcoder-tui"

echo ""
echo "=== Done ==="
echo "  mcoder     -> $BINDIR/mcoder"
echo "  mcoder-tui -> $BINDIR/mcoder-tui"
echo ""
echo "Usage:  mcoder        (start server + TUI)"
echo "        mcoder server  (server only)"
echo "        mcoder tui     (TUI only)"
