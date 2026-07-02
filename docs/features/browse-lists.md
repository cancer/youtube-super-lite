# 一覧（おすすめ / 登録新着 / 履歴 / 再生リスト）

対象読者: 一覧オーバーレイの操作方法とデータソースの違いを確認したい人。

## Why

URL を都度貼り付けるだけでは「次に何を見るか」を選べず、ブラウザに戻って探す羽目になる。このアプリは
YouTube本家と同じ主要な発見導線（おすすめ・登録チャンネル・履歴・再生リスト）を全画面オーバーレイとして
内蔵し、ブラウザに切り替えずに次の動画を選べるようにしている。

## 使い方

上部ナビゲーション帯のタブ（`📋 おすすめ` / `📃 再生リスト` / `📺 登録チャンネル` / `🕘 履歴`）を
クリックすると、そのソースの一覧が全画面オーバーレイで開く（キーボードなら `Tab` で開き、`1`〜`4` で
ソース切替。[controller-ui.md](controller-ui.md)）。サムネイル付きの一覧をマウスのクリックまたは
`↑`/`↓` + `Enter` で選んで再生する。右上の `✕` ボタン（キーボードなら `Esc`）で閉じる。

サムネイルは自前取得してディスクキャッシュし、WIC でデコードする（詳細:
[ui-settings-and-gpu.md](ui-settings-and-gpu.md) の画像キャッシュ節）。

## データソースの違い

| ソース | 取得元 | 認証 | 備考 |
|------|------|------|------|
| おすすめ | 視聴中動画のウォッチページ HTML 内 `ytInitialData` の `secondaryResults` | 不要 | 専用APIではなくウォッチページのパースで取得。API クォータを消費しない |
| 登録チャンネル新着 | InnerTube `browse?browseId=FEsubscriptions`（TVHTML5 + OAuth Bearer） | OAuth | 新着フィードのみ。特定チャンネルのアップロード一覧へのドリルダウンは未実装 |
| 履歴 | InnerTube `browse?browseId=FEhistory`（TVHTML5 + OAuth Bearer） | OAuth | WEB クライアントは OAuth Bearer を拒否するため TVHTML5 を使用 |
| 再生リスト | Data API v3 `playlists.list` → `playlistItems.list` | OAuth | 詳細は [playlists.md](playlists.md) |

OAuth が必要なタブ（再生リスト/登録チャンネル/履歴）は、未ログインの間はナビゲーション帯に
表示されない（[login-and-rating.md](login-and-rating.md)）。

登録新着・履歴はどちらも InnerTube のタイル構造（`tileRenderer`）から `contentId`（動画ID）・サムネ・
タイトル・チャンネル名を同じ形で読み取る。登録新着は複数の棚（shelf）に同じ動画が重複して出ることがあるため
動画IDで重複排除する。

## 関連

- [playlists.md](playlists.md) — 再生リストの詳細（特別枠プレイリストの扱いなど）
- [login-and-rating.md](login-and-rating.md) — OAuth が必要な一覧を使うためのログイン設定
