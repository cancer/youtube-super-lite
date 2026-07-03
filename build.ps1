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

[string[]]$profileArgs = if ($Release) { @("--release") } else { @() }
$outDir      = if ($Release) { "release" } else { "debug" }

# Import vcvars64 environment into this PowerShell session, then call cargo directly.
# (Going through `cmd /c "... && cargo build"` can drop cargo's exit code, so a
#  successful build is mis-detected as "failed" and the DLL copy step gets skipped.
#  Calling cargo from PowerShell sets $LASTEXITCODE reliably.)
cmd /c "`"$vcvars`" >nul 2>&1 && set" | ForEach-Object {
    if ($_ -match '^([^=]+)=(.*)$') { Set-Item -Path "env:$($matches[1])" -Value $matches[2] }
}
$env:MPV_SOURCE = $mpvSrc

# --workspace: 本体に加え resolver-sidecar(gated 用 rustypipe 解決器)も同じ target に出す。
Push-Location $root
& $cargo build @profileArgs --workspace
$code = $LASTEXITCODE
Pop-Location
if ($code -ne 0) { Write-Error "cargo build failed (exit $code)"; exit $code }

# 実行時に必要なファイルを exe の隣へコピー（解決は native InnerTube + サイドカーに移行済み = yt-dlp 不要）
$dest = Join-Path $root "target\$outDir"
Copy-Item (Join-Path $mpvSrc "libmpv-2.dll") $dest -Force
# resolver-sidecar.exe は同じ target\$outDir に出るので追加コピー不要（本体 exe から spawn される）。

$sidecar = Join-Path $dest "resolver-sidecar.exe"
$hasSidecar = Test-Path $sidecar
Write-Host "Build OK ($outDir). libmpv-2.dll copied. resolver-sidecar present: $hasSidecar" -ForegroundColor Green
exit 0
