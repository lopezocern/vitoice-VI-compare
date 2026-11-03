# 启动 VideoCC（开发模式）
#
# 为什么把 target 目录放到工作区外：
#   本机 `G:\Code\vitoice_videoCC` 目录上残留了 DSH 沙箱注入的 ACL（Everyone DENY DELETE
#   + 一条 S-1-4-... 的沙箱 SID ACE）。WebView2 在该目录下初始化渲染进程会返回
#   HRESULT 0x800700AA (ERROR_BUSY)，窗口起不来。构建产物放到工作区外即可正常运行。
#
#   如果想改回默认（产物在 src-tauri/target），删掉下面这行即可，但当前目录下会失败。

$ErrorActionPreference = 'Stop'
$env:CARGO_TARGET_DIR = 'G:\Code\videocc-target'

Set-Location (Join-Path $PSScriptRoot 'src-tauri')
Write-Host "构建目录: $env:CARGO_TARGET_DIR" -ForegroundColor DarkGray
cargo run
