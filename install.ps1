#!/usr/bin/env pwsh
# mcoder 一键构建安装脚本 (Windows PowerShell)
# 安装到 %USERPROFILE%\.cargo\bin\（需 Rust + Bun）
# 用法: .\install.ps1
$ErrorActionPreference = "Stop"

$ROOT = Split-Path -Parent $MyInvocation.MyCommand.Path
Write-Host "=== mcoder installer ===" -ForegroundColor Cyan

# 检查依赖
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { Write-Host "ERROR: cargo not found. Install Rust: https://rustup.rs" -ForegroundColor Red; exit 1 }
if (-not (Get-Command bun -ErrorAction SilentlyContinue))  { Write-Host "ERROR: bun not found. Install: powershell -c `"irm bun.sh/install.ps1 | iex`"" -ForegroundColor Red; exit 1 }
if (-not (Get-Command npm -ErrorAction SilentlyContinue))  { Write-Host "ERROR: npm not found. Install Node.js first." -ForegroundColor Red; exit 1 }

# 1. 构建 mcoder (Rust)
Write-Host "`n[1/3] Building mcoder (Rust)..." -ForegroundColor Yellow
Push-Location "$ROOT\mcoder"
cargo install --path . --force
Pop-Location

# 2. 构建 mcoder-tui (Bun compile 单文件)
Write-Host "`n[2/3] Building mcoder-tui (Bun standalone)..." -ForegroundColor Yellow
Push-Location "$ROOT\mcoder-tui"
# 删除 lockfile 重新生成，避免 package.json/lockfile 不一致导致 peer dep 冲突
# （历史上 react-devtools-core 在 package.json 写 ^7 但 lockfile 锁 4，npm install 时升级会与 React 18 冲突）
if (Test-Path package-lock.json) {
    Remove-Item package-lock.json -Force
}
if (Test-Path node_modules) {
    Remove-Item node_modules -Recurse -Force
}
npm install --legacy-peer-deps --silent
npm run build
# Patch ink-picture
$inkPicture = "node_modules\ink-picture\build\components\image\index.js"
if (Test-Path $inkPicture) {
    $content = Get-Content $inkPicture -Raw
    if ($content -match "useIsScreenReaderEnabled") {
        Write-Host "Patching ink-picture for Bun compatibility..."
        $content = $content -replace 'import \{ Box, useIsScreenReaderEnabled, useStdout \} from "ink";', 'import { Box, useStdout } from "ink";'
        $content = $content -replace 'import React, \{ useMemo \} from "react";', 'import React, { useMemo, useState } from "react";'
        $content = $content -replace 'const isScreenReaderEnabled = useIsScreenReaderEnabled\(\);', 'const isScreenReaderEnabled = useState(false)[0];'
        Set-Content $inkPicture $content
    }
}
bun build --compile dist/index.js --outfile mcoder-tui.exe
Pop-Location

# 3. 复制到同目录
Write-Host "`n[3/3] Installing..." -ForegroundColor Yellow
$BINDIR = "$env:USERPROFILE\.cargo\bin"
if (-not (Test-Path $BINDIR)) { New-Item -ItemType Directory -Path $BINDIR -Force }
Copy-Item "$ROOT\mcoder-tui\mcoder-tui.exe" "$BINDIR\mcoder-tui.exe" -Force

Write-Host "`n=== Done ===" -ForegroundColor Green
Write-Host "  mcoder     -> $BINDIR\mcoder.exe"
Write-Host "  mcoder-tui -> $BINDIR\mcoder-tui.exe"
Write-Host "`nUsage:  mcoder        (start server + TUI)"
Write-Host "        mcoder server  (server only)"
Write-Host "        mcoder tui     (TUI only)"
