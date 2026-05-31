# Talava Player build helper (Windows / MSVC)
# - MSVC 環境 (vcvars64) を読み込み
# - libmpv の場所 (MPV_SOURCE) を設定
# - cargo build を実行し、実行に必要な DLL/exe を出力先へコピー
# 使い方:  .\build.ps1            (debug build)
#          .\build.ps1 -Release   (release build)
param([switch]$Release)

$root = $PSScriptRoot
$vcvars = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
$cargo  = "$env:USERPROFILE\.cargo\bin\cargo.exe"
$mpvSrc = Join-Path $root "tools\mpv-dev"

if (-not (Test-Path $vcvars))          { Write-Error "vcvars64.bat not found: $vcvars"; exit 1 }
if (-not (Test-Path "$mpvSrc\mpv.lib")) { Write-Error "mpv.lib not found in $mpvSrc";   exit 1 }

$profileArg = if ($Release) { "--release" } else { "" }
$outDir     = if ($Release) { "release" } else { "debug" }

# vcvars + MPV_SOURCE を設定して cargo build。失敗時はその終了コードで抜ける。
cmd /c "`"$vcvars`" >nul 2>&1 && set MPV_SOURCE=$mpvSrc && cd /d `"$root`" && `"$cargo`" build $profileArg"
$code = $LASTEXITCODE
if ($code -ne 0) { Write-Error "cargo build failed (exit $code)"; exit $code }

# 実行時に必要なファイルを exe の隣へコピー
$dest = Join-Path $root "target\$outDir"
Copy-Item (Join-Path $mpvSrc "libmpv-2.dll") $dest -Force
$ytdlp = Join-Path $root "tools\yt-dlp.exe"
if (Test-Path $ytdlp) { Copy-Item $ytdlp $dest -Force }

Write-Host "Build OK ($outDir). libmpv-2.dll / yt-dlp.exe copied to target\$outDir" -ForegroundColor Green
exit 0
