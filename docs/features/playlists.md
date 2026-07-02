# 再生リスト

対象読者: 再生リスト一覧・中身の表示ロジックを確認したい人。

## Why

再生リストは「後で見る」「シリーズものをまとめて見る」といった用途で使われるが、中身が多いリストは
一覧・中身の2階層に分けて見た方が把握しやすい。このアプリの再生リスト表示はその2階層構造を
そのままオーバーレイに落とし込んでいる。

## 使い方

上部ナビゲーション帯の `📃 再生リスト` をクリック（ログイン時のみ表示）すると再生リスト一覧
（1階層目）が開く。項目をクリック、または `↑`/`↓` + `Enter` で中身（2階層目）を開く。中身から
一覧へ戻るには右上の `✕` か `Backspace` を使う（詳細: [browse-lists.md](browse-lists.md)）。
認証（OAuth）が必要。

## データソース

- 一覧: Data API v3 `playlists.list?mine=true`
- 中身: Data API v3 `playlistItems.list?playlistId=...`（`pageToken` でページング）
- 「後で見る」「高く評価した動画」などの特別プレイリストは、`channels.list?part=contentDetails&mine=true`
  の `contentDetails.relatedPlaylists` から取得したIDを使い、ユーザー作成の再生リストより先頭に表示する
  （本家 YouTube でこれらが特別扱いされているのと同じ体験にするため）

## 関連

- [browse-lists.md](browse-lists.md) — 一覧オーバーレイ全体の操作
- [login-and-rating.md](login-and-rating.md) — OAuth 設定手順
