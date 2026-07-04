# アーキテクチャ概要

対象読者: このコードベースを初めて触る人、モジュール間の責務分担を確認したい人。

## モジュールマップ

```mermaid
graph TD
    main["main.rs<br/>CLI解析 → NativeApp起動<br/>Quality/Codec は ysl-core::types から再エクスポート"]
    ui_shell["ui::shell::NativeRunning (shell)<br/>winitアプリ本体・全ドメイン状態を所有<br/>HWND→mpv埋め込み・キーボード/状態・各種poll"]
    overlay["dcomp_overlay<br/>子窓+DirectCompositionオーバーレイ<br/>一覧・URL欄・チャットを描画"]
    flows["ysl_core::flows<br/>跨ぎ system 3本(on_logged_in/play/play_with_chat)"]
    account["ysl_core::account<br/>credentials(アプリ寿命) + AuthTask(per-op)"]
    playback["ysl_core::playback<br/>装置/好み(アプリ寿命) + PlaySession(1URL:1個)"]
    content["ysl_core::content<br/>Feed×3 / ChannelView / Playlist / AvatarCache"]
    chat["ysl_core::chat<br/>ChatSession(1動画:1個)"]
    player["ysl_core::player::Player<br/>libmpvラッパー<br/>mpvをwidにD3D11埋め込み"]
    resolve["ysl_core::yt::resolve<br/>ネイティブInnerTubeリゾルバ<br/>常駐ワーカー + sidecarフォールバック"]
    yt["ysl_core::yt::*<br/>auth/recommend/subscriptions/history/playlist/chat/mark_watched"]
    image_cache["ysl_core::image_cache<br/>サムネのディスクキャッシュ"]
    settings["settings<br/>UI設定(チャット文字サイズ・幅)の永続化"]
    gpu_usage["ysl_core::gpu_usage<br/>GPU使用率監視とHW/SWデコード自動切替"]
    devtools["devtools<br/>検証用ローカルHTTPサーバ(--enable-dev-tools)"]

    main --> ui_shell
    ui_shell --> overlay
    ui_shell --> devtools
    ui_shell --> flows & account & playback & content & chat
    flows --> account & playback & content & chat
    playback --> resolve & player & gpu_usage
    account & content & chat --> yt
    playback --> yt
    ui_shell --> image_cache & settings
```

機能ごとの詳細は [docs/features/](../features/) を参照。ここでは横断的な設計方針だけを扱う。

## 設計原則

### 1. UI 非依存コア（crates/ysl-core）

状態とロジックはドメイン(`account`/`playback`/`content`/`chat`)+ 跨ぎ処理(`flows`)に分かれ、
フロントエンド（描画/入力）から分離されている。lib(`crates/ysl-core`)は winit に依存しないため、
UI フレームワークの型がコアへ漏れることをコンパイラが拒否する。状態の**所有**は shell
（`ui::shell::NativeRunning`）が担い、各ドメインの `Feed`/`Account`/`Playback`/`ChatSession` 等を
個別フィールドとして保持する（束ねる「God struct」は作らない）。旧 `Controller` はこの再編で消滅した。

各ドメインが持つ主な責務:
- `playback`: mpv (`Player`) の制御（再生/一時停止/シーク/画質切替/HWデコード切替）・URL 解決のディスパッチ
- `account`: 認証状態（トークン・チャンネル名）とその API 呼び出し
- `flows::play`: 起動直後は「サイレントログイン完了まで解決を保留する」レースガードを持ち、
  匿名解決がメンバー限定動画をロックしてしまうのを防ぐ（跨ぎ処理はここにしか置けない。3本のみ）
- `content`: 各一覧（おすすめ/登録新着/履歴/再生リスト）のバックグラウンド取得結果の受信
- `chat`: ライブチャットのバックグラウンド取得結果の受信（1 動画 : 1 セッション）

詳細な設計判断（データ/振る舞いの分離・情報隠蔽・依存の明示・ID 参照）は
[design-principles.md](design-principles.md) を参照。

### 2. 描画とレンダリングの分離

動画は mpv が D3D11 でウィンドウへ直接描画する。UI（コントローラ・一覧・チャット）は別の透過子窓に
DirectComposition 経由で描画する。**両者は GPU コンテキストを共有しない**。チャット表示時は mpv の
`video-margin-ratio-right` プロパティで動画の描画領域自体を左に縮め、空いた右側にチャットパネルを描く
（オーバーレイの重ね描きではなく、真の左右分割）。

レンダリング方式そのものの設計判断（なぜ DirectComposition か、なぜ OpenGL/egui を廃止したか）は
[overlay-rendering.md](overlay-rendering.md) を参照。

### 3. I/O はすべてバックグラウンドスレッド + mpsc

API 呼び出し・URL解決・チャットポーリング・サムネ取得など、ブロッキングしうる処理はすべて別スレッドで実行し、
結果は `std::sync::mpsc::channel` でメインスレッドへ送る。lib(`crates/ysl-core`)は winit を知らないため、
背景スレッド完了の通知は `Waker`（`Arc<dyn Fn() + Send + Sync>`）という抽象を介す。bin 側（shell）が
winit の `EventLoopProxy::send_event(UserEvent::Background)` を包んだクロージャを `Waker` として各ドメインの
`start_*` に注入する。これにより winit のメインループは常駐スレッドを一切ブロックせず、CPU バウンドな
イベント処理に専念できる。

各バックグラウンド系（認証・チャット・おすすめ・登録新着・履歴・再生リスト・URL解決）は独立したチャンネルを持ち、
`ui::shell::NativeRunning`（shell）がそれぞれの `poll_*` をイベントループの tick ごとに回す。

### 4. 撤去済みの旧設計

- **OpenGL Render API + egui**: 単一 GL コンテキストでの合成方式。起動時の OpenGL ドライバ bring-up が
  他アプリの GPU 再生を一瞬妨げる問題があり撤去。詳細は [inbox/opengl-to-native-migration.md](../../inbox/opengl-to-native-migration.md)。
- **`WS_EX_LAYERED` + `UpdateLayeredWindow` オーバーレイ**（旧 `native_overlay.rs`）: 子窓+DirectComposition
  方式に置き換えて撤去。詳細は [overlay-rendering.md](overlay-rendering.md)。
- **yt-dlp.exe の逐次起動によるURL解決**: 常駐ネイティブリゾルバに置き換えて撤去（バイナリも配布物から除去済み）。
  詳細は [url-resolution.md](url-resolution.md)。

## 関連ドキュメント

- [url-resolution.md](url-resolution.md) — ネイティブ InnerTube リゾルバの設計
- [overlay-rendering.md](overlay-rendering.md) — DirectComposition オーバーレイの設計
- [auth-backend.md](auth-backend.md) — OAuth と Cloudflare Worker バックエンドの設計
- [threading-and-io.md](threading-and-io.md) — スレッドモデルと mpsc 配線の詳細
- [shell-checklist.md](shell-checklist.md) — `src/ui/shell.rs` 変更時の手動チェックリスト（地雷3件）
