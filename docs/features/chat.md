# ライブチャット

対象読者: チャット表示の挙動・データソース・制限を確認したい人。

## 概要

| 項目 | 内容 |
|------|------|
| 操作 | `Ctrl+T` で開閉 |
| 認証 | 不要 |
| データソース | InnerTube `live_chat/get_live_chat`（配信中）/ `live_chat/get_live_chat_replay`（配信終了後） |
| 表示位置 | 動画を左に縮め、右側にパネル表示（真の左右分割。[design/overlay-rendering.md](../design/overlay-rendering.md)） |

## 取得の仕組み

視聴中動画のウォッチページから `ytInitialData` を取得し、`INNERTUBE_API_KEY` と継続トークン
（`conversationBar.liveChatRenderer` 以下）を抽出する。配信中かどうかは `isReplay` フラグで判定し、
配信中なら `get_live_chat`、終了後（アーカイブ）なら `get_live_chat_replay` をポーリングする。

アーカイブ再生時は現在の再生位置（`player_offset_ms`）を継続的にバックグラウンドスレッドへ共有し、
再生位置に応じたコメントを取得する。継続トークンは `reloadContinuationData` /
`timedContinuationData` の両方に対応する。

チャット表示のオン/オフに追従してポーリングを即座に止められるよう、単純な `sleep` ではなく
インタラプト可能な待機を使う（[design/threading-and-io.md](../design/threading-and-io.md)）。

## 表示内容

- 発言者バッジ（オーナー / モデレーター / 認証済み / メンバー）を `authorBadges` から判定して表示
- カスタム絵文字（メンバーシップスタンプ等）は画像をインライン描画する。デコード前は alt テキストを表示
- チャットの文字サイズ・パネル幅は `Ctrl+ -`/`Ctrl+ +` や左端ドラッグで変更でき、[設定として永続化](ui-settings-and-gpu.md)される

## 保持件数

パネルに保持するメッセージ数には上限がある（メモリ上のリングバッファ的な扱い）。

## 関連

- [ui-settings-and-gpu.md](ui-settings-and-gpu.md) — 文字サイズ・幅の永続化
- [devtools.md](devtools.md) — `toggle_chat` / `chat_font_inc` 等の外部操作
