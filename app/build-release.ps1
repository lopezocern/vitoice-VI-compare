# 构建发布版（性能版）
#
# 开发时的 debug 构建解码图片/视频会慢 10 倍以上，日常使用请用这个。
# 默认关闭 LTO 以缩短构建时间（运行性能差异在 5% 以内）。

$ErrorActionPreference = 'Stop'
if (-not $env:CARGO_TARGET_DIR) {
    $outside = 'G:\Code\videocc-release'
    if (Test-Path (Split-Path $outside -Parent)) {
        $env:CARGO_TARGET_DIR = $outside
    }
}
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path (Join-Path $PSScriptRoot 'src-tauri') 'target' }
$env:CARGO_PROFILE_RELEASE_LTO = 'false'
$env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS = '16'

Set-Location (Join-Path $PSScriptRoot 'src-tauri')
Write-Host "构建目录: $target (release)" -ForegroundColor DarkGray
cargo build --release

$exe = Join-Path $target 'release\vitoicecompare.exe'
if (Test-Path $exe) {
    $mb = [math]::Round((Get-Item $exe).Length / 1MB, 1)
    Write-Host "`n构建完成: $exe ($mb MB)" -ForegroundColor Green
    if ($args -contains '-Run') {
        Start-Process $exe
    }
}
