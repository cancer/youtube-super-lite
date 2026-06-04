# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

YouTube Super Lite is a Rust YouTube player for Windows and macOS: libmpv renders video into an OpenGL framebuffer and egui draws the UI on top. A Cloudflare Worker (`auth-worker/`) holds the OAuth client secret so the desktop binary never ships it.

## Building / running

### macOS

Requires `mpv` and `yt-dlp` from Homebrew:

```sh
brew install mpv yt-dlp
cargo build               # debug
cargo build --release     # release
```

`build.rs` runs `pkg-config --libs-only-L mpv` to find the libmpv link path. `yt-dlp` is picked up from `PATH` by `ensure_ytdlp_on_path`.

Run:
```sh
./target/debug/youtube-super-lite "https://www.youtube.com/watch?v=..."
./target/debug/youtube-super-lite -v                            # verbose
./target/debug/youtube-super-lite --debug-backend http://127.0.0.1:8787  # override backend
```

### Windows

`cargo build` **does not work on its own.** Linking libmpv needs the MSVC environment and a link-search path that the crates don't set. Always build via:

```powershell
.\build.ps1            # debug
.\build.ps1 -Release   # release
```

`build.ps1` loads `vcvars64.bat`, sets `MPV_SOURCE=tools\mpv-dev`, runs `cargo build`, then copies `libmpv-2.dll` and `yt-dlp.exe` next to the exe (both are required at runtime and are git-ignored). If you must call cargo directly, replicate that environment.

Run:
```powershell
.\target\debug\youtube-super-lite.exe "https://www.youtube.com/watch?v=..."
```

### Verification

There is **no test suite**. Verification is done by launching the app (window appears) and grepping stderr for `AV:` / `[frames]` progression with `--verbose`.

### Toolchain prerequisites
- **macOS**: Rust + `brew install mpv yt-dlp`. `pkg-config` (bundled with Homebrew formulae).
- **Windows**: Rust **MSVC** toolchain + Visual Studio Build Tools 2022 (VC Tools). `tools/mpv-dev/` (libmpv dev package from shinchiro/mpv-winbuild-cmake) and `tools/yt-dlp.exe`. `libmpv-2.dll` and `yt-dlp.exe` are git-ignored — re-fetch them when cloning fresh (see README).
- For `auth-worker/`: Node 18+, `wrangler`, the `wasm32-unknown-unknown` target. On Windows PowerShell's execution policy blocks `npx.ps1`; invoke wrangler through `cmd /c "... npx wrangler ..."`.

## CLI arguments

```
youtube-super-lite [OPTIONS] [URL]
  -v, --verbose       mpv の詳細ログを出力
      --debug-backend URL   認証バックエンドを上書き（デバッグ用。デフォルト: 本番Worker）
      --enable-dev-tools    ローカル検証用 HTTP サーバを起動（後述の GUI 検証フロー）
  -h, --help          ヘルプを表示
```

Environment variables are not used for app configuration; debugging knobs are CLI flags.

## UIスタック選定の背景とトレードオフ

現在の構成（egui + egui_glow + OpenGL）は、UI として GPU 描画を使うことが本質的に必要だから選んだものではなく、「**クロスプラットフォーム単一コードで素早く書きたい**」を最優先したトレードオフの結果。

### GPU 描画（OpenGL）で得ているもの
- 唯一の本質的メリット: **mpv の動画描画と同じ GL コンテキスト / フレームバッファを共有して、UI を動画上に直接合成できる**
- 副次的: HiDPI スケール、egui の immediate mode 描画の効率

### 代わりに失っているもの
- **アクセシビリティツリー（AXUIElement 等）非対応** — OS の自動化 API からは window と traffic light しか見えない。`cliclick` のような画面座標ベースの操作しかできず、E2E テストはマシン全体の入力を占有する
- ネイティブ UI の見た目・挙動（IME / フォント / テーマ）との不整合
- GL コンテキスト初期化のオーバーヘッド、ドライバ差異

### 「OpenGL は 3D 規格では？」への回答
歴史的にはそう。現在は GPU 描画一般の API として 2D UI でも広く使われる（egui / ImGui / mpv / VLC など）。3D の枠組み（頂点・テクスチャ・シェーダ）で 2D の四角形にテクスチャを貼る形で利用している。

### この選択が壊れる/再考すべき条件
- **配布製品化フェーズに入る場合**: アクセシビリティ・テスト容易性・OS ネイティブ UX を得るために、IINA 型（ネイティブ UI + mpv 子プロセス）への移行検討に値する
- **E2E テストを本格運用する場合**: UI ロジックをライブラリ化し、モック版バイナリでテスト（src/bin/youtube-super-lite-uitest.rs 等を別途用意）

現状の egui + libmpv 構成は **プロトタイプ / 機能完成度を上げるフェーズには適している**が、製品化フェーズで再評価が必要なポイント。

## libmpv linking (the fragile part)

- The crate `libmpv2-sys` only emits `cargo:rustc-link-lib=mpv`; it does **not** add a search path. [build.rs](build.rs) resolves the path per-platform: `pkg-config` on macOS, `tools/mpv-dev` on Windows.
- `tools/mpv-dev/mpv.lib` (Windows) is an MSVC import library generated from the DLL's exports (`dumpbin /exports libmpv-2.dll` → `mpv.def` → `lib /def:... /name:libmpv-2.dll`). Regenerate it (steps in README) if the DLL changes.
- **`libmpv2` must stay on 6.x.** 4.x crashes (`0xC0000005`) at `RenderContext` creation due to a use-after-free in the Render API path.

## Rendering architecture ([src/main.rs](src/main.rs))

- winit 0.30 `ApplicationHandler`. All live state lives in `Running`, created in `App::init` on `resumed`.
- glutin creates the GL context; its `get_proc_address` feeds **both** glow (for egui_glow) and the mpv `RenderContext` (which is owned by `player::Player`). mpv uses `vo=libmpv` (not `gpu`/`wid`).
- 動画は **`Player` 内部の FBO/テクスチャ**に描画される（既定 FBO=0 ではなく）。`gl_quad::FullscreenQuad` がそのテクスチャを既定 FBO に全画面描画し、egui がその上に UI を重ねる。`UI` を差し替えるとき `Player` には触らずに済むよう分離してある（詳細: [UIスタック選定の背景とトレードオフ](#uiスタック選定の背景とトレードオフ) 節）。
- **`Mpv` is `Box::leak`'d to `'static`** so `RenderContext<'static>` (which borrows `Mpv`) can be stored in `Player` without a self-referential type.
- Redraws are driven by mpv's update callback → `EventLoopProxy<UserEvent>` (`MpvRedraw`) → `request_redraw`. Background threads (auth / chat / recommend / subs / playlist / resolve) wake the loop with `UserEvent::Background`. `ControlFlow::Wait` otherwise.
- UI auto-hide: panels are skipped (and the cursor hidden) after `UI_HIDE_AFTER` of no mouse/key activity. Because mpv doesn't drive redraws while paused, `redraw` self-requests the next frame when `paused && show_ui` so the hide countdown still advances; vsync caps that loop.
- チャット表示中は動画 viewport を `(0, 0, w - CHAT_PANEL_WIDTH * scale_factor, h)` に縮めて、右側に egui の `SidePanel::right` でチャットを並べる（重ねない）。詳細は **物理ピクセル vs 論理ポイント** の注意（後述）。

## yt-dlp / DASH 処理 ([src/resolve.rs](src/resolve.rs))

- mpv 同梱の ytdl_hook（Lua）は終了済みライブ配信などで yt-dlp の JSON 出力が肥大化（数十 MB）するとパース失敗するため、**ytdl_hook は無効化**（`init.set_property("ytdl", false)`）してアプリ側で yt-dlp を直接呼ぶ。
- `yt-dlp -g -f "bestvideo+bestaudio/best"` でストリーム URL を取得。
- URL が DASH manifest（`manifest.googlevideo.com/api/manifest/dash/...`）なら **[`dash-mpd`](https://crates.io/crates/dash-mpd)** クレートで MPD XML をパースし、SegmentTemplate の `$Number$` / `$Time$` / `$RepresentationID$` / `$Bandwidth$` を展開してセグメント URL 列を生成、mpv EDL（`edl://!mp4_dash,init=<init>;<seg1>;<seg2>;...`）に変換して `loadfile` に渡す。
- それ以外（短尺動画の直リンク、ライブ HLS）は URL をそのまま `loadfile`。
- タイトルは `yt-dlp --print "%(title)s"` で別取得し、`loadfile` の `force-media-title=` オプションで mpv に注入。
- yt-dlp は `ensure_ytdlp_on_path` で PATH 上を `which` 検索 → 同梱 dir フォールバックの順で見つける。

## Auth architecture ([src/auth.rs](src/auth.rs) + [auth-worker/](auth-worker/))

The desktop app **never holds `client_secret`.** Only the browser consent step is delegated to the browser; everything else is in-app API calls.

- Login: `auth::login` runs a loopback OAuth code flow (`TcpListener` on `127.0.0.1:<port>`, opens the consent URL via `crate::open_in_browser`, captures the redirect). The authorization-code/refresh exchange is POSTed to the **Cloudflare Worker** (`/token`, `/refresh`), which appends `client_id`/`client_secret` and relays to Google. The app defaults to the production Worker (`auth::DEFAULT_BACKEND`); `--debug-backend` overrides it for local development.
- API calls that need only an access token (`videos.rate` for like, `channels?mine=true` for the name) are made directly from the app.
- Refresh token persists at platform-specific paths and the app auto-logs-in on startup if present:
  - Windows: `%APPDATA%\YouTubeSuperLite\auth.json`
  - macOS: `~/Library/Application Support/YouTubeSuperLite/auth.json`
- All network runs on spawned threads; results return via `mpsc` (`AuthMsg`) and are drained in `redraw`/`poll_auth`. **egui closures only set intent flags** (`login_clicked`, `like_clicked`, `to_load`); the actual `start_login`/`start_like`/`load` run *after* `egui_glow.run` returns, because `egui_glow` is borrowed mutably during the closure. Follow this pattern when adding UI actions that touch `self`.

## Features ([src/chat.rs](src/chat.rs), [src/recommend.rs](src/recommend.rs), [src/subscriptions.rs](src/subscriptions.rs), [src/playlist.rs](src/playlist.rs))

- **Live chat** (`chat.rs`): InnerTube `get_live_chat` polling. OAuth not required. Right side panel overlay.
- **Recommendations** (`recommend.rs`): InnerTube watch page `ytInitialData.secondaryResults`. OAuth not required. Full-screen overlay.
- **Subscription feed** (`subscriptions.rs`): YouTube Data API v3 `subscriptions.list?mine=true` (OAuth) + per-channel RSS feeds (no quota). Full-screen overlay.
- **My playlists** (`playlist.rs`): YouTube Data API v3 `playlists.list?mine=true` and `playlistItems.list`. OAuth required. Two-stage overlay (list → items).

### The Worker ([auth-worker/src/lib.rs](auth-worker/src/lib.rs), workers-rs)
- Endpoints: `GET /client_id`, `POST /token`, `POST /refresh`. Reads var `GAUTH_CLIENT_ID` and secret `GAUTH_CLIENT_SECRET`.
- Deploy from `auth-worker/`: set `GAUTH_CLIENT_ID` in `wrangler.jsonc`, `wrangler secret put GAUTH_CLIENT_SECRET`, then `wrangler deploy` (the `build` command compiles Rust→WASM via `worker-build`).
- The Google OAuth client must be type **Desktop app** (loopback `127.0.0.1` redirect) with scope `youtube.force-ssl`, and the user must be a test user on the consent screen.

## 作業者として注意すべき事項

### コーディング上の落とし穴

#### 物理ピクセル vs 論理ポイント（Retina）
- `winit::Window::inner_size()` は **物理ピクセル** を返す。Retina で 2 倍。
- egui の各種サイズ指定（`exact_width`、`max_height` 等）は **論理ポイント (dp)**。
- OpenGL の `viewport(x, y, w, h)` は **物理ピクセル**。
- mpv の `RenderContext::render(fbo, w, h, flip)` も **物理ピクセル**（GL FBO サイズと一致させる）。
- **不一致のまま計算すると Retina で半分しかスケールしないバグになる**（実際に発生済み）。サイドパネル幅などウィンドウから引き算する値は `window.scale_factor()` をかけて物理ピクセル単位に揃えること。

#### egui クロージャ内での self 借用
- `egui_glow.run(window, |ctx| { ... })` の中では `egui_glow` が `&mut self` として借用されているため、`self` の他フィールドを mutate しようとすると借用エラーになる。
- UI イベント（ボタンクリック等）は **intent flag (`login_clicked: bool` 等)** をクロージャ内で立てるだけにし、実際の処理（`self.start_login()` 等）は `egui_glow.run` の戻り後に実行する。新しい UI アクションを追加するときも同じパターンに揃えること。

#### 背景スレッドの結果は mpsc で受ける
- すべての I/O（OAuth・API・yt-dlp・チャットポーリング・画像 DL）はバックグラウンドスレッドで実行。
- 結果は `mpsc::channel` でメインスレッドに送り、`redraw` で `poll_xxx()` ですくう。
- 背景スレッド完了時は `EventLoopProxy::send_event(UserEvent::Background)` でイベントループを叩き起こす（`ControlFlow::Wait` なので必須）。

#### YouTube 絵文字の扱い
- `liveChatTextMessageRenderer.message.runs[].emoji` で 2 種類:
  - 標準 Unicode 絵文字: `isCustomEmoji: false`、`emojiId` に Unicode 文字（例 "🔥"）。テキストとして挿入し、egui の絵文字フォント（Apple Color Emoji 等）で描画。
  - カスタム絵文字（メンバーシップスタンプ）: `isCustomEmoji: true`、`image.thumbnails[].url` に PNG/WEBP の画像 URL。`egui::Image::new(url)` + `egui_extras::install_image_loaders` で動的取得・描画。
- shortcut 文字列（`:_mikoGood:` 等）にフォールバックするのは画像 URL も emojiId も無い極端なケースだけ。

#### ライブ vs リプレイ
- `ytInitialData.contents.twoColumnWatchNextResults.conversationBar.liveChatRenderer.isReplay` で判定可能。
- リプレイの場合 `get_live_chat` ではなく `get_live_chat_replay` エンドポイントを使う必要がある（現状未対応）。
- リプレイは `subMenuItems` 内に「上位のチャット」「すべてのチャット」の 2 種類の continuation を持つ。

#### フォントフォールバック順
- `FontDefinitions::default()` には egui 同梱の `Ubuntu-Light` / `Hack` / `NotoEmoji-Regular` / `emoji-icon-font` が入っている。
- ここに **絵文字フォント → 日本語フォント** の順で append すると、CJK 範囲外の絵文字 codepoint で先に絵文字フォントが hit し、日本語フォントが絵文字用に誤マッチしない。逆順にすると CJK 文字が絵文字フォントの不適切なグリフで描画される可能性。

### 環境・コマンド操作上の注意

#### `pkill -f` は危険
- `pkill -f youtube-super-lite` は **引数を含むコマンドライン全体** にマッチするため、`tmux attach-session -t youtube-super-lite` のようなセッション名一致だけのプロセスまで殺してしまう。
- 代わりに以下を使う:
  - `pkill -x youtube-super-lite`（プロセス名完全一致のみ。引数は見ない）
  - `kill $APP_PID`（具体的な PID）
  - `pgrep -x youtube-super-lite | xargs kill`

#### macOS の GUI 自動操作には Accessibility 権限が必要
- `cliclick` / `osascript` での System Events 操作（マウス移動・クリック・キー送信）には **アクセシビリティ権限**（オートメーション権限ではない）が必要。
- 権限はユーザーが手動で許可する必要があり、付与した親プロセス（kitty / iTerm / claude 等）を再起動しないと反映されないことがある。
- スクリーンキャプチャ自体（`screencapture`）は権限不要だが、**ディスプレイがスリープ中は黒画像を返す**ため、画面が黒い → アプリ異常とは限らない。`caffeinate -u -t N` で起動チェック可能。

#### egui 0.29 の API
- `egui::Frame::new()` は **存在しない**。`Frame::none()` を使う。
- `ScrollArea` はデフォルトでコンテンツに合わせて縮むため、固定パネル内で使うときは `.auto_shrink([false, false])` を付ける。
- フルスクリーン覆い被せのオーバーレイは `egui::Area::new(...).order(Order::Foreground).fixed_pos(screen.min)` + 中に `Frame.show` で実装。Frame の `inner_margin` を入れた場合は中の `ui.set_max_size(screen.size() - vec2(margin*2, margin*2))` でないと右下が画面外に押し出される。
- `ui.with_layout(right_to_left, Align::Center)` を単独で使うと **利用可能エリア全体（高さも含む）** に展開され、上下中央揃えで配置される。1 行ぶんの高さに収めるには **`ui.horizontal()` で囲む** こと。

#### Cargo / 依存関係
- `libmpv2` は **6.x 固定**（4.x は Render API でクラッシュ）。
- `egui_glow` は `features = ["winit"]`、`egui-winit` は `features = ["clipboard"]` を必須に（後者は明示的にクリップボード機能を有効化しないと Cmd+V でペーストできない）。
- `egui_extras` は `features = ["image", "http"]`、`image` クレートは `features = ["png", "webp", "jpeg"]`。PNG/WEBP はカスタム絵文字、JPEG は YouTube 動画サムネ（`i.ytimg.com/.../mqdefault.jpg`）に必要。features から外すと該当画像が ⚠ で表示される。

### GUI 検証フロー

**推奨: `--enable-dev-tools` 経由**。アプリ内 HTTP サーバから back buffer を PNG で直接取れるので、フォーカス奪取・Accessibility 権限・スリープ中の黒画像といった `screencapture` 由来の落とし穴を全部回避できる。詳細な仕様は [README.md](README.md) の「開発者向けツール」節を参照。

```sh
./target/debug/youtube-super-lite --enable-dev-tools "<URL>" 2>/tmp/yt.stderr &
APP_PID=$!
# dev-tools サーバ起動 + 動画ロード完了まで待つ。短い動画なら 6〜10 秒で十分。
# 「再生準備中」など中央オーバーレイの解除前にスクショを撮ると、それも含めて
# 写るので問題ない（むしろアプリ状態が一目で分かる）。
sleep 8
PORT=$(grep -oE 'http://127.0.0.1:[0-9]+' /tmp/yt.stderr | head -1 | grep -oE '[0-9]+$')
curl -sS -o /tmp/shot.png "http://127.0.0.1:$PORT/screenshot"
# 検証後
kill $APP_PID
```

- **真っ黒な PNG が返った場合は実際に画面が黒い**。状態は中央オーバーレイで可視化されているはずなので、本当に何も描かれていない＝アプリ異常のサイン（mpv が動いていない、redraw が止まっている等）。`screencapture` のときのような「スリープ中だから黒」「OS の合成抑制で黒」は dev-tools 経由では起きない（back buffer を直接読むため OS コンポジットの前段）。
- **UI 操作の検証は `POST /action/<name>` で intent flag を立てられる**。利用可能アクション: `toggle_chat` / `toggle_recommend` / `toggle_subs` / `toggle_playlist` / `toggle_history` / `play_pause` / `login` / `like` / `close_overlay`。
- **任意座標のクリックは `POST /click?x=<px>&y=<px>`**（座標は `/screenshot` と同じ物理ピクセル）。egui に合成ポインタ（移動→押下→離す）を注入するので、動画クリックでの再生/一時停止やボタンも検証できる。`cliclick` フォールバックは不要。

```sh
# 例: 履歴オーバーレイを開いてスクショ
curl -sS -X POST "http://127.0.0.1:$PORT/action/toggle_history"
sleep 1
curl -sS -o /tmp/history.png "http://127.0.0.1:$PORT/screenshot"
```

#### フォールバック: `screencapture` / `cliclick`

dev-tools サーバが起動しない（ポート競合等）、または UI 操作が必要なときの代替。

1. アプリを `&` でバックグラウンド起動し PID を控える
2. `osascript -e 'tell application "System Events" to set frontmost of (first process whose name is "youtube-super-lite") to true'` で前面化
3. `cliclick m:x,y` で UI を起こす（auto-hide 解除）
4. `screencapture -x -R<screen_x>,<screen_y>,<w>,<h> /tmp/shot.png` でキャプチャ
5. 検証後は `kill $APP_PID` で停止

注意点: `cliclick c:x,y` のクリック検証はマシン全体の入力を占有するため、テスト中はユーザーが操作不可。Accessibility 権限が必要（前述）。配布製品化フェーズでは `egui_kittest` 等の UI 単体テスト基盤を別途用意する想定（UI スタック節参照）。
