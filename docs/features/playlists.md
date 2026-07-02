# 再生リスト

対象読者: 再生リスト一覧・中身の表示ロジックを確認したい人。

## 概要

一覧オーバーレイで `4` を押すと再生リスト一覧（1階層目）が表示され、選択して `Enter` で中身
（2階層目）を開く。`Backspace` で一覧へ戻る。認証（OAuth）が必要。

## データソース

- 一覧: Data API v3 `playlists.list?mine=true`
- 中身: Data API v3 `playlistItems.list?playlistId=...`（`pageToken` でページング）
- 「後で見る」「高く評価した動画」などの特別プレイリストは、`channels.list?part=contentDetails&mine=true`
  の `contentDetails.relatedPlaylists` から取得したIDを使い、ユーザー作成の再生リストより先頭に表示する

## 関連

- [browse-lists.md](browse-lists.md) — 一覧オーバーレイ全体の操作
- [login-and-rating.md](login-and-rating.md) — OAuth 設定手順
