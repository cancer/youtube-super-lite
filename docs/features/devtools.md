# dev-tools（検証用ローカル HTTP）

対象読者: UI操作の自動検証・スクリーンショット取得を行いたい人（人手のマウス/キー操作を使わずに）。

## 起動方法

```powershell
.\target\debug\youtube-super-lite.exe --enable-dev-tools
```

外部の screencapture / クリックツールに依存せず、アプリ自身がローカル HTTP でスクリーンショット撮影・
状態取得・UI 操作を受け付ける。起動時に listen ポートを stderr に表示する
（`[dev-tools] http://127.0.0.1:<port> ...`）。`curl` だけで検証フローを回せ、**あらゆる UI 操作を
グローバル入力なしで駆動でき、状態も観測できる**（SendKeys 等のグローバル入力は他ウィンドウに
誤爆するため使わない）。

## 実装メモ

`tiny_http` によるリクエスト受信は専用スレッドで行い、受けたコマンドは mpsc チャンネル経由で
メインスレッドに転送、winit の `EventLoopProxy` で処理を起こしてから応答を返す（1リクエストあたり
5秒タイムアウト）。詳細な設計は [design/threading-and-io.md](../design/threading-and-io.md)。

## エンドポイント

| メソッド / パス | 説明 |
|------|------|
| `GET /screenshot` | 現在のウィンドウ（クライアント領域）を PNG で返す。撮影前にウィンドウを前面化し、オーバーレイ込みの合成画を取得する。**注意**: 画面座標の BitBlt のため、他ウィンドウが前面に重なっていると写り込むことがある |
| `GET /state` | 現在の UI 状態スナップショットを JSON で返す（`paused` / `volume` / `muted` / `quality` / `codec` / `is_live` / `chat_open` / `chat_font_px` / `list_*` / `logged_in` 等） |
| `POST /action/<name>` | UI 操作を起こす（下記） |
| `POST /click?x=&y=` | クライアント px 座標に左クリックを注入（コントロール矩形へ振り分け） |
| `POST /type`（body=text, `?enter=1`） | URL 欄へテキスト入力。`enter=1` で再生 |

## `/action/<name>` 一覧

- 再生: `play_pause` / `seek_fwd` / `seek_back` / `live_edge`
- 音量: `vol_up` / `vol_down` / `mute`
- 画質・コーデック: `quality_next` / `codec_next`
- チャット: `toggle_chat` / `chat_font_inc` / `chat_font_dec` / `chat_wider` / `chat_narrower`
- 認証・評価: `login` / `like`
- URL: `play_url`（URL 欄の内容を再生）
- 一覧: `toggle_list` / `close_overlay` / `open_recommend` / `open_subs` / `open_playlist` / `open_history` / `list_up` / `list_down` / `list_select` / `list_back`

## 関連

- [controller-ui.md](controller-ui.md) — 同じ操作の通常キーバインド対応表
