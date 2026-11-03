# 构建发布版（性能版）
#
# 开发时的 debug 构建解码图片/视频会慢 10 倍以上，日常使用请用这个。
# 默认关闭 LTO 以缩短构建时间（运行性能差异在 5% 以内）。

$ErrorActionPreference = 'Stop'
$env:CARGO_TARGET_DIR = 'G:\Code\videocc-release'
$env:CARGO_PROFILE_RELEASE_LTO = 'false'
$env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS = '16'

Set-Location (Join-Path $PSScriptRoot 'src-tauri')
Write-Host "构建目录: $env:CARGO_TARGET_DIR (release)" -ForegroundColor DarkGray
cargo build --release

$exe = Join-Path $env:CARGO_TARGET_DIR 'release\videocc.exe'
if (Test-Path $exe) {
    $mb = [math]::Round((Get-Item $exe).Length / 1MB, 1)
    Write-Host "`n构建完成: $exe ($mb MB)" -ForegroundColor Green
    if ($args -contains '-Run') {
        Start-Process $exe
    }
}
