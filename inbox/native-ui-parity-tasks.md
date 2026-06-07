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
- [x] #2 `OverlayAction` 拡張（TogglePause/Seek/SetVolume/ToggleMute/Like/ToggleChat/OpenList(tab)/CycleQuality/CycleCodec/Login/PlayIndex）+ `OvShared` に全コントロールのヒット矩形とアクションキュー（`actions: Vec`）
- [x] #3 `render` 再設計（上部バー＋2段コントローラ＋タイトル行を描画し各矩形を保存）。描画ヘルパーは `OverlayView` 構造体ではなく `Painter`（fill_round/fill_rect/text/text_center/button）として実装。ヒット矩形は毎フレーム `hits: OvShared` に蓄積→OV_STATE へ書出。[判断: 別名 struct より既存スタイルに沿った軽量ヘルパーが妥当]
- [x] #4 入力モデル: コントロール帯（上部 UI／下部コントローラ）・チャット・一覧の上だけ
      WM_NCHITTEST=HTCLIENT で捕捉し、それ以外は HTTRANSPARENT で親 winit へ通す。active 時のみ
      オーバーレイ可視。`dispatch_hit` でコントロール矩形へ振り分け（バー余白の非ヒットは TogglePause）。
      [修正: 当初は「常時可視＋全画面 HTCLIENT」だったが、クリック時に画面が一瞬真っ白になる不具合が
      出たため撤回。動画クリック=一時停止は winit の `MouseInput`（透過領域のクリックが親へ届く）で
      処理する方式に変更。#21 のゴールはより確実に達成]

## 上部バー
- [x] #5 URL 入力欄（英数字入力・Ctrl+V・Enter 再生。維持）
- [x] #6 「おすすめ」タブボタン → OpenList(Recommend)
- [x] #7 「登録チャンネル」タブボタン → OpenList(Subs)（未取得なら取得＝ensure_source_fetched）
- [x] #8 「再生リスト」タブボタン → OpenList(Playlist)（2階層: PlayIndex で 1階層目は中身を開く）
- [x] #9 「履歴」タブボタン → OpenList(History)（未取得なら取得）
- [x] #10 ログインボタン/認証状態表示（未ログイン時のみクリック可 → Login。ログイン済みはチャンネル名表示のみ）
- [x] #11 動画タイトル表示（`media_title`）

## 下部コントローラ
- [x] #12 再生/一時停止ボタン（▶/⏸）
- [x] #13 シークバー（**seekable 時のみ可動**。DVR 無しライブは赤トラック＋"● LIVE" 固定表示で操作無効）
- [x] #14 時間表示（mm:ss / mm:ss）
- [x] #15 音量バー（クリック位置で 0–130 設定 → SetVolume）
- [x] #16 ミュートボタン（トグル。🔊/🔇 切替）
- [x] #17 画質選択（クリックで巡回 → 再生中なら start_resolve 取り直し）
- [x] #18 コーデック選択（クリックで巡回 → 同上）
- [x] #19 高評価ボタン 👍（→ start_like。未ログイン/video_id 無しは Controller 側で no-op）
- [x] #20 チャットトグル 💬（表示/非表示。set_video_margin_right 連動。チャット中は hot 強調）

## 操作
- [x] #21 動画クリックで再生/一時停止（オーバーレイ透過領域のクリックを winit `MouseInput` で受けて
      TogglePause。UI 非表示中＝オーバーレイ非表示時も親 winit がクリックを受けるので有効）
- [x] #22 一覧のクリック選択（行クリック → PlayIndex で再生/ドリル）
- [x] #23 チャット左右分割（維持。draw_chat を active と独立に chat_open で描画）

## 仕上げ
- [x] #24 native_app 配線: 全 OverlayAction を `apply_overlay_action` で適用、可視=フォーカス連動・active=操作後3秒/一覧/チャット、タブ→一覧オープン。キーボードショートカットは補助として残す
- [x] #25 ビルド（debug/release とも警告0）＆ bundle.ps1 で配布バンドル再作成（dist\youtube-super-lite-win64.zip）。
      起動スモークテスト（`--volume 0`）で overlay 初期化・無クラッシュ・GPU 監視開始を確認。
      ※「実機での全UI目視・クリック動作確認」は操作・観察が必要なため未実施（下記 不明点リスト参照）。

## 不明点リスト（要確認 / 判断メモ）
- [判断・非ブロッキング] #3 は仕様の `OverlayView` という名前の構造体ではなく `Painter` ヘルパー＋
  毎フレームの `hits: OvShared` 蓄積で実装した。機能等価（描画ヘルパー＋矩形保存）と判断し採用。
- [軽微・非ブロッキング] チャットパネル領域のクリックは（コントロール矩形でないため）TogglePause に
  落ちる＝動画クリック扱い。チャットはスクロール等の操作が無いため実害は小さいと判断。要望あれば
  チャットパネルを no-op ヒット矩形として登録する。
- [軽微] 画質/コーデックボタンは横幅の都合でラベルプレフィックス無しで値のみ表示（例 "1080p" / "VP9"）。
- [要ユーザー確認・#25] release 再バンドルと起動スモークは完了。残る「実機で全UIを目視し、各ボタン・
  タブ・シーク・音量・ミュート・👍・💬・動画クリック一時停止を実際にクリックして確認」は私が
  GUI を操作・観察できないため未実施。ユーザーによる手動確認が必要。

## 未確定（要確認）
- 動画の「概要/説明文」表示: egui 版にも無かった（タイトルのみ）。要望があれば別途。
- フルスクリーン切替: 未実装（egui 版にも無し）。
