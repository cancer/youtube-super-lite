# 視聴履歴の記録（YouTube側への反映）

対象読者: 「YouTube 本家の履歴/継続視聴にこのアプリでの再生が反映される仕組み」を確認したい人。

## Why

このアプリで動画を見ても、YouTube本家サイトの視聴履歴や「続きから再生」に何も反映されないと、
本家のブラウザ/スマホアプリと併用したときに視聴体験が分断されてしまう。この機能は、このアプリでの
再生も本家アカウントの履歴に積み上げることで、視聴デバイスを問わず一貫した体験にするためのもの。

## 仕組み

ログイン時に限り、再生開始時・一定時間経過時に YouTube 側の視聴統計へ ping を送る（yt-dlp の
`_mark_watched` ロジックを踏襲した実装）。

1. InnerTube `player` エンドポイント（TVHTML5 クライアント + OAuth Bearer）を叩き、レスポンスの
   `playbackTracking.videostatsPlaybackUrl.baseUrl` / `videostatsWatchtimeUrl.baseUrl` を取得する
2. ランダムな CPN（Client Playback Nonce、16文字）を生成し、`ver` / `cpn` / `cmt`（再生位置）/
   `el` / `st` / `et` などのパラメータを付けて、上記の base URL に GET リクエストを送る
3. 正常時は `204 No Content` が返る

**失敗しても再生自体には影響しない**（ベストエフォート）。ユーザーが意識して使う機能ではなく、
裏側で自動的に動く。

## 前提条件

ログインしていない場合はこの機能自体が動作しない（OAuth Bearer が必須のため）。

## 関連

- [login-and-rating.md](login-and-rating.md) — ログイン設定
- [browse-lists.md](browse-lists.md) — 履歴一覧側（読み出し）との違い（こちらは書き込み）
