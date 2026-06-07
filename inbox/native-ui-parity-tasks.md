# タスク: ネイティブ版UIの機能 parity（egui版の可視・クリック操作を全移植）

> 背景: P4 で egui を撤去したネイティブ版は、可視のクリック可能 UI（音量/ミュート/タイトル/高評価/
> チャットトグル/各タブ/動画クリック停止）が欠落し、キーボード操作頼みで parity 未達だった。
> さらに mute/seekable/media_title を dead code として誤削除していた。本タスクで egui 版の
> UI 仕様（commit ef43669 の redraw 参照）をネイティブ(Direct2D オーバーレイ)へ可視・操作可能に移植する。
>
> 仕様の出典（egui 版 UI）:
> - 上部バー: URL欄 / タブ[おすすめ・登録チャンネル・再生リスト・履歴] / ログイン・認証状態 / 動画タイトル
> - 下部コントローラ: シーク(seekable時のみ可動) / 再生・一時停止 / 時間 / 音量バー / ミュート /
>   画質 / コーデック / 👍高評価 / 💬チャット
> - 動画クリックで再生/一時停止（UI 非表示中も有効）

## 基盤
- [x] #1 Player 状態 API の復活（`muted` / `set_muted` / `seekable` / `media_title`）
- [ ] #2 `OverlayAction` 拡張（TogglePause/Seek/SetVolume/ToggleMute/Like/ToggleChat/OpenList(tab)/CycleQuality/CycleCodec/Login/PlayListIndex）+ `OvShared` に全コントロールのヒット矩形とアクションキュー
- [ ] #3 `OverlayView` 構造体導入 + `render` 再設計（上部バー＋2段コントローラ＋タイトル行を描画し各矩形を保存。描画ヘルパー fill_round/draw_text/button）
- [ ] #4 入力モデル: フォーカス中は常時可視・WM_NCHITTEST=HTCLIENT で全クリック捕捉。アイドル時(active=false)はコントロール非描画でも矩形を空にして「動画クリック=一時停止」を成立。WM_LBUTTONDOWN で矩形ヒットを各 Action に振り分け、非ヒットは TogglePause

## 上部バー
- [ ] #5 URL 入力欄（英数字入力・Ctrl+V・Enter 再生。維持）
- [ ] #6 「おすすめ」タブボタン → OpenList(Recommend)
- [ ] #7 「登録チャンネル」タブボタン → OpenList(Subs)（未取得なら取得）
- [ ] #8 「再生リスト」タブボタン → OpenList(Playlist)（2階層）
- [ ] #9 「履歴」タブボタン → OpenList(History)（未取得なら取得）
- [ ] #10 ログインボタン/認証状態表示（未ログイン時クリックで Login → start_login）
- [ ] #11 動画タイトル表示（`media_title`）

## 下部コントローラ
- [ ] #12 再生/一時停止ボタン（▶/⏸。維持）
- [ ] #13 シークバー（**seekable 時のみ可動**。DVR 無しライブは固定表示で操作無効＝現状の「DVR無効でもシークが動く」修正）
- [ ] #14 時間表示（mm:ss / mm:ss。維持）
- [ ] #15 音量バー（クリック位置で 0–130 設定）
- [ ] #16 ミュートボタン（トグル。🔊/🔇 切替）
- [ ] #17 画質選択（クリックで巡回 → 再生中なら start_resolve 取り直し）
- [ ] #18 コーデック選択（クリックで巡回 → 同上）
- [ ] #19 高評価ボタン 👍（ログイン中＋再生中のみ有効 → start_like）
- [ ] #20 チャットトグル 💬（表示/非表示。動画の右マージン連動）

## 操作
- [ ] #21 動画クリックで再生/一時停止（コントロール矩形外。UI 非表示中も有効）
- [ ] #22 一覧のクリック選択（行クリックで再生/ドリル。維持）
- [ ] #23 チャット左右分割（動画を左へ縮小し右にチャットパネル。維持）

## 仕上げ
- [ ] #24 native_app 配線: 全 OverlayAction を Player/Controller に適用、OverlayView 構築、可視/active ロジック、タブ→一覧オープン。キーボードショートカットは補助として残す
- [ ] #25 ビルド（警告0）＆実機で全UIの表示・クリック動作を確認 → release 再バンドル

## 未確定（要確認）
- 動画の「概要/説明文」表示: egui 版にも無かった（タイトルのみ）。要望があれば別途。
- フルスクリーン切替: 未実装（egui 版にも無し）。
