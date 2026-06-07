# 配布用バンドル作成（Windows / native 版）。
# release ビルド → 実行に必要なファイルを dist\youtube-super-lite\ にまとめ、zip 化する。
#
# 使い方:
#   .\bundle.ps1              # release ビルドしてからバンドル
#   .\bundle.ps1 -SkipBuild   # 既存の target\release を使ってバンドルのみ
param([switch]$SkipBuild)

$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$name = 'youtube-super-lite'

if (-not $SkipBuild) {
    # build.ps1 は最後に exit するため、子プロセスで実行して当スクリプトを巻き込まない。
    & pwsh -NoProfile -File (Join-Path $root 'build.ps1') -Release
    if ($LASTEXITCODE -ne 0) { Write-Error "release build failed (exit $LASTEXITCODE)"; exit 1 }
}

$rel = Join-Path $root 'target\release'
$exe = Join-Path $rel "$name.exe"
if (-not (Test-Path $exe)) { Write-Error "exe not found: $exe （先に release ビルドが必要）"; exit 1 }

$dist  = Join-Path $root 'dist'
$stage = Join-Path $dist $name
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

# 実行に必要なファイル（exe・libmpv・yt-dlp）。
$files = @("$name.exe", 'libmpv-2.dll', 'yt-dlp.exe')
foreach ($f in $files) {
    $src = Join-Path $rel $f
    if (Test-Path $src) { Copy-Item $src $stage -Force }
    else { Write-Warning "見つからないので同梱しません: $f" }
}
# 簡単な実行手順を同梱。
@"
YouTube Super Lite (Windows / native)

使い方:
  youtube-super-lite.exe ["https://www.youtube.com/watch?v=..."]
  引数なしで起動し、英数字キーで URL 入力（Ctrl+V 貼り付け）→ Enter で再生。

同梱物:
  youtube-super-lite.exe … 本体
  libmpv-2.dll           … 再生エンジン (mpv)
  yt-dlp.exe             … YouTube ストリーム解決

操作・ログイン設定は README を参照:
  https://github.com/cancer/youtube-super-lite
"@ | Set-Content -Path (Join-Path $stage 'README.txt') -Encoding UTF8

$zip = Join-Path $dist "$name-win64.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -Force

Write-Host "bundle dir: $stage" -ForegroundColor Green
Write-Host "zip       : $zip" -ForegroundColor Green
Get-ChildItem $stage | Select-Object Name, @{n='KB';e={[int]($_.Length/1KB)}}
