# 再生検証ハーネス（解決器が「実際に mpv で再生できるか」を機械判定する）
#
# 背景: 「URL 解決 + curl 206」では実運用の再生可否を保証できない（PoToken 等で
# 解決はできても mpv 取得時に 403 になる）。本スクリプトは実アプリを各動画で起動し、
# **mpv の再生時計(AV:)が 0 を超えて進行したか** を成功条件として判定する。
#
# 使い方:
#   pwsh scripts/verify_playback.ps1                  # target/release を既定の動画群で検証
#   pwsh scripts/verify_playback.ps1 -Exe target/debug/youtube-super-lite.exe
#   pwsh scripts/verify_playback.ps1 -Videos dQw4w9WgXcQ,9bZkp7q19f0
#
# 終了コード: 全動画が再生できれば 0、1つでも失敗/不明があれば 1（CI ゲート用）。

param(
  [string]$Exe = "target/release/youtube-super-lite.exe",
  [string[]]$Videos = @(
    "dQw4w9WgXcQ",  # Rick Astley（定番・軽い）
    "jNQXAC9IVRw",  # 初投稿（古い・短い）
    "9bZkp7q19f0",  # PSY Gangnam（音楽/VEVO）
    "kJQP7kiw5Fk",  # Despacito（音楽/VEVO）
    "OPf0YbXqDm0",  # Mark Ronson Uptown Funk（音楽/VEVO）
    "2Vv-BfVoq4g",  # Ed Sheeran Perfect（音楽/VEVO）
    "M7lc1UVf-VE"   # Google Developers（一般）
  ),
  [int]$WaitSec = 10
)

$root = Split-Path -Parent $PSScriptRoot
$exePath = if ([System.IO.Path]::IsPathRooted($Exe)) { $Exe } else { Join-Path $root $Exe }
if (-not (Test-Path $exePath)) { Write-Error "exe が見つかりません: $exePath（先に build.ps1 を実行）"; exit 2 }

Write-Host "=== 再生検証: $exePath ===" -ForegroundColor Cyan
Write-Host "判定: mpv の AV: 時計が 00:00:00 を超えて進行 = ▶再生OK / 403・解決失敗 = ✗`n"

$results = @()
foreach ($id in $Videos) {
  $log = Join-Path $env:TEMP "verify_$id.log"
  if (Test-Path $log)        { Remove-Item $log -Force }
  if (Test-Path "$log.err")  { Remove-Item "$log.err" -Force }

  $p = Start-Process -FilePath $exePath `
        -ArgumentList @("https://www.youtube.com/watch?v=$id", "--verbose", "--volume", "0") `
        -WorkingDirectory $root -RedirectStandardOutput $log -RedirectStandardError "$log.err" -PassThru
  Start-Sleep -Seconds $WaitSec

  $lines = @()
  if (Test-Path $log)       { $lines += Get-Content $log -ErrorAction SilentlyContinue }
  if (Test-Path "$log.err") { $lines += Get-Content "$log.err" -ErrorAction SilentlyContinue }

  # 解決した client（loadfile 行の c= から）
  $client = ""
  $lf = $lines | Where-Object { $_ -like 'loadfile:*' } | Select-Object -First 1
  if ($lf) { $client = [regex]::Match($lf, '[?&]c=([^&]+)').Groups[1].Value }

  # 再生進行: AV: の位置が 00:00:00 以外（=1秒以上進んだ）の行があるか
  $advanced = $false
  $lastAv = ""
  foreach ($l in ($lines | Where-Object { $_ -like 'AV:*' })) {
    $lastAv = $l
    $m = [regex]::Match($l, 'AV:\s+(\d\d):(\d\d):(\d\d)')
    if ($m.Success) {
      $sec = [int]$m.Groups[1].Value * 3600 + [int]$m.Groups[2].Value * 60 + [int]$m.Groups[3].Value
      if ($sec -ge 1) { $advanced = $true }
    }
  }
  $failed = ($lines | Where-Object { $_ -match 'Failed to open|resolve failed|HTTP error 403' } | Select-Object -First 1)

  $status = if ($advanced) { "▶OK" } elseif ($failed) { "✗FAIL" } else { "?UNKNOWN" }
  $note = if ($advanced) { ($lastAv -replace '\s+', ' ' -replace 'A-V.*', '').Trim() } elseif ($failed) { ($failed -replace '\s+', ' ').Trim() } else { "AV行なし" }
  $results += [pscustomobject]@{ Video = $id; Client = $client; Status = $status; Note = $note }

  Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
  Start-Sleep -Milliseconds 300
}

Write-Host ("{0,-14} {1,-11} {2,-9} {3}" -f "Video", "Client", "Status", "Note")
Write-Host ("-" * 80)
foreach ($r in $results) {
  $color = switch ($r.Status) { "▶OK" { "Green" } "✗FAIL" { "Red" } default { "Yellow" } }
  Write-Host ("{0,-14} {1,-11} {2,-9} {3}" -f $r.Video, $r.Client, $r.Status, $r.Note) -ForegroundColor $color
}

$ok = ($results | Where-Object { $_.Status -eq "▶OK" }).Count
$total = $results.Count
Write-Host "`n=== 結果: $ok / $total 再生成功 ===" -ForegroundColor Cyan
if ($ok -eq $total) { exit 0 } else { exit 1 }
