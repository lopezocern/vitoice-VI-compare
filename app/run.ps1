# 启动 VitoiceCompare（开发模式）
#
# 构建目录默认放在工作区外，便于避开某些机器上沙箱注入的 ACL
# （Everyone DENY DELETE 会导致 WebView2 初始化失败，报 HRESULT 0x800700AA）。
# 没有 G: 盘的机器会自动退回默认的 src-tauri\target；
# 也可以用环境变量 CARGO_TARGET_DIR 自行指定。

$ErrorActionPreference = 'Stop'
if (-not $env:CARGO_TARGET_DIR) {
    $outside = 'G:\Code\videocc-target'
    if (Test-Path (Split-Path $outside -Parent)) {
        $env:CARGO_TARGET_DIR = $outside
    }
}
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path (Join-Path $PSScriptRoot 'src-tauri') 'target' }

Set-Location (Join-Path $PSScriptRoot 'src-tauri')
Write-Host "构建目录: $target" -ForegroundColor DarkGray
cargo run
