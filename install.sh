#!/bin/bash
# mcoder 一键构建安装脚本（macOS/Linux）
# 安装到 ~/.cargo/bin/（需 Rust + Bun）
# 用法: ./install.sh
set -e

ROOT="$(cd "$(dirname "$0")" && pwd)"

echo "=== mcoder installer ==="

# ===== 0. PATH 自愈（解决 ~/.cargo/bin / Xcode 工具链不在 PATH 的问题）=====
# 常见原因：用户从 IDE / Finder / ssh 启动 shell 时 PATH 不完整
# 注意：必须在 PATH 前部加入标准系统路径，避免覆盖 /usr/bin
export PATH="$HOME/.cargo/bin:/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin:$PATH"

# Xcode Command Line Tools 自愈：cc/clang 不在 PATH 时，从 /Applications/Xcode.app 找
if ! command -v cc >/dev/null 2>&1; then
  for xcode in /Applications/Xcode.app /Applications/Xcode-beta.app; do
    if [ -d "$xcode/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin" ]; then
      export PATH="$xcode/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin:$PATH"
      export DEVELOPER_DIR="$xcode/Contents/Developer"
      break
    fi
  done
fi

# SDK 自愈：xcrun 不存在 / SDK 找不到时，用 sdk_path 直接设 SDKROOT
if ! command -v xcrun >/dev/null 2>&1; then
  for xcode in /Applications/Xcode.app /Applications/Xcode-beta.app; do
    for sdk in "$xcode/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk" \
               "$xcode/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX26.5.sdk" \
               "$xcode/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX26.sdk"; do
      if [ -d "$sdk" ]; then
        export SDKROOT="$sdk"
        break 2
      fi
    done
  done
fi

# ===== 1. 检查依赖 =====
command -v cargo >/dev/null || { echo "ERROR: cargo not found. Install Rust: https://rustup.rs"; exit 1; }
command -v bun  >/dev/null || { echo "ERROR: bun not found. Install: curl -fsSL https://bun.sh/install | bash"; exit 1; }
command -v npm  >/dev/null || { echo "ERROR: npm not found. Install Node.js first."; exit 1; }

echo "  cargo:  $(command -v cargo)"
echo "  bun:    $(command -v bun)"
echo "  npm:    $(command -v npm)"
echo "  cc:     $(command -v cc 2>/dev/null || echo '(none)')"
echo "  SDKROOT: ${SDKROOT:-(none)}"

# 2. 构建 mcoder (Rust)
echo ""
echo "[1/3] Building mcoder (Rust)..."
cd "$ROOT/mcoder"
cargo install --path . --force

# 3. 构建 mcoder-tui (Bun compile 单文件)
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

# 4. 复制到同目录
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