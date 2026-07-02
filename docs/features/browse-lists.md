# 一覧（おすすめ / 登録新着 / 履歴 / 再生リスト）

対象読者: 一覧オーバーレイの操作方法とデータソースの違いを確認したい人。

## 概要

`Tab` キーで全画面の一覧オーバーレイを開閉する。`1`/`2`/`3`/`4` でソースを切り替え、`↑`/`↓` またはクリックで
項目を選択、`Enter` で再生する。サムネイルは自前取得してディスクキャッシュし、WIC でデコードする
（詳細: [ui-settings-and-gpu.md](ui-settings-and-gpu.md) の画像キャッシュ節）。

## データソースの違い

| ソース | キー | 取得元 | 認証 | 備考 |
|------|------|------|------|------|
| おすすめ | `2` | 視聴中動画のウォッチページ HTML 内 `ytInitialData` の `secondaryResults` | 不要 | 専用APIではなくウォッチページのパースで取得。API クォータを消費しない |
| 登録チャンネル新着 | `1` | InnerTube `browse?browseId=FEsubscriptions`（TVHTML5 + OAuth Bearer） | OAuth | 新着フィードのみ。特定チャンネルのアップロード一覧へのドリルダウンは未実装 |
| 履歴 | `3` | InnerTube `browse?browseId=FEhistory`（TVHTML5 + OAuth Bearer） | OAuth | WEB クライアントは OAuth Bearer を拒否するため TVHTML5 を使用 |
| 再生リスト | `4` | Data API v3 `playlists.list` → `playlistItems.list` | OAuth | 詳細は [playlists.md](playlists.md) |

登録新着・履歴はどちらも InnerTube のタイル構造（`tileRenderer`）から `contentId`（動画ID）・サムネ・
タイトル・チャンネル名を同じ形で読み取る。登録新着は複数の棚（shelf）に同じ動画が重複して出ることがあるため
動画IDで重複排除する。

## 関連

- [playlists.md](playlists.md) — 再生リストの詳細（特別枠プレイリストの扱いなど）
- [login-and-rating.md](login-and-rating.md) — OAuth が必要な一覧を使うためのログイン設定
