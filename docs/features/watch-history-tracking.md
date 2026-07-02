# 視聴履歴の記録（YouTube側への反映）

対象読者: 「YouTube 本家の履歴/継続視聴にこのアプリでの再生が反映される仕組み」を確認したい人。

## 概要

このアプリで動画を再生すると、ログイン時に限り YouTube 側の視聴統計へ ping を送り、本家サイトの
視聴履歴・継続視聴（「続きから再生」）に反映されるようにする。yt-dlp の `_mark_watched` ロジックを
踏襲した実装（`mark_watched.rs`）。

## 仕組み

1. InnerTube `player` エンドポイント（TVHTML5 クライアント + OAuth Bearer）を叩き、レスポンスの
   `playbackTracking.videostatsPlaybackUrl.baseUrl` / `videostatsWatchtimeUrl.baseUrl` を取得する
2. ランダムな CPN（Client Playback Nonce、16文字）を生成し、`ver` / `cpn` / `cmt`（再生位置）/
   `el` / `st` / `et` などのパラメータを付けて、上記の base URL に GET リクエストを送る
3. 正常時は `204 No Content` が返る

再生開始時・一定時間経過時に呼ばれる。**失敗しても再生自体には影響しない**（ベストエフォート）。

## 前提条件

ログインしていない場合はこの機能自体が動作しない（OAuth Bearer が必須のため）。

## 関連

- [login-and-rating.md](login-and-rating.md) — ログイン設定
- [browse-lists.md](browse-lists.md) — 履歴一覧側（読み出し）との違い（こちらは書き込み）
