#!/bin/bash
# 构建 mcoder-tui 单文件可执行（Bun compile）
# 前置：npm install && npm run build
set -e

cd "$(dirname "$0")"

# Patch: ink-picture 引用了 ink 中不存在的 useIsScreenReaderEnabled
# 用 useState(false) 替代（screen reader 检测在终端中无实际用途）
INK_PICTURE="node_modules/ink-picture/build/components/image/index.js"
if grep -q "useIsScreenReaderEnabled" "$INK_PICTURE" 2>/dev/null; then
  echo "Patching ink-picture for Bun compatibility..."
  sed -i.bak 's/import { Box, useIsScreenReaderEnabled, useStdout } from "ink";/import { Box, useStdout } from "ink";/' "$INK_PICTURE"
  sed -i.bak 's/import React, { useMemo } from "react";/import React, { useMemo, useState } from "react";/' "$INK_PICTURE"
  sed -i.bak 's/const isScreenReaderEnabled = useIsScreenReaderEnabled();/const isScreenReaderEnabled = useState(false)[0];/' "$INK_PICTURE"
  rm -f "${INK_PICTURE}.bak"
  echo "Patched."
fi

# Bun compile -> 单文件可执行（自带 Bun runtime，无 Node.js 依赖）
echo "Compiling with Bun..."
bun build --compile dist/index.js --outfile mcoder-tui

echo "Done: $(ls -lh mcoder-tui | awk '{print $5}') mcoder-tui"
