# 构建 Windows 安装包（NSIS）
#
# 产物：<CARGO_TARGET_DIR>\release\bundle\nsis\VitoiceCompare_<版本>_x64-setup.exe
# 首次运行需要 NSIS（Tauri 会自动下载到 %LOCALAPPDATA%\tauri\NSIS）

$ErrorActionPreference = 'Stop'

if (-not $env:CARGO_TARGET_DIR) {
    $outside = 'G:\Code\videocc-release'
    if (Test-Path (Split-Path $outside -Parent)) {
        $env:CARGO_TARGET_DIR = $outside
    }
}
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path (Join-Path $PSScriptRoot 'src-tauri') 'target' }
# 关闭 LTO 缩短构建时间（运行性能差异在 5% 以内）
if (-not $env:CARGO_PROFILE_RELEASE_LTO) {
    $env:CARGO_PROFILE_RELEASE_LTO = 'false'
    $env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS = '16'
}

Set-Location $PSScriptRoot
Write-Host "构建目录: $target" -ForegroundColor DarkGray

tauri build --bundles nsis

$setup = Get-ChildItem (Join-Path $target 'release\bundle\nsis') -Filter '*-setup.exe' |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1

if ($setup) {
    Write-Host ""
    Write-Host ("安装包: {0}  ({1:N1} MB)" -f $setup.FullName, ($setup.Length / 1MB)) -ForegroundColor Green
} else {
    Write-Warning '未找到生成的安装包'
}
