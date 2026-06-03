# YouTube Super Lite

Rust 製の YouTube プレーヤー（Windows / macOS）。

- 再生エンジン: **libmpv (mpv)** の Render API（OpenGL）
- ウィンドウ / GL コンテキスト: **winit** + **glutin**
- 操作 UI: **egui**（動画と並べてサイドパネル、操作系は重ねて表示）
- YouTube URL の解決: **yt-dlp**（mpv の ytdl_hook ではなくアプリ側で直接呼ぶ）
- DASH manifest の処理: **`dash-mpd`** クレートでパースし mpv EDL に変換（ffmpeg に DASH demuxer がなくても再生可能）

## 主な機能

| 機能 | 説明 | 認証 |
|------|------|------|
| 動画再生 | 短尺動画・配信中ライブ・終了ライブのアーカイブをすべて再生 | 不要 |
| 高評価 | ログイン中ユーザーで `videos.rate` を呼んで高評価を付ける | OAuth |
| ライブチャット | InnerTube `get_live_chat` 経由でメッセージを取得し動画の右側にサイドパネル表示 | 不要 |
| メンバーシップスタンプ | カスタム絵文字を YouTube CDN から動的に取得してインライン画像描画 | 不要 |
| おすすめ動画 | ウォッチページの `ytInitialData.secondaryResults` から関連動画一覧をオーバーレイ表示 | 不要 |
| 登録チャンネル新着 | Data API v3 で登録チャンネル一覧を取り、各チャンネルの RSS フィードから新着動画を集約 | OAuth |
| 自分の再生リスト | Data API v3 `playlists.list?mine=true` → `playlistItems.list` で 2 段階オーバーレイ表示 | OAuth |

## アーキテクチャ

```
[main.rs / Running]
  ├ player::Player          — libmpv ラッパー。動画を内部 FBO/テクスチャに描画
  ├ gl_quad::FullscreenQuad — テクスチャを画面に描画するクワッド
  ├ egui_glow::EguiGlow     — UI 描画
  └ 各機能モジュール（chat / recommend / subscriptions / playlist / resolve / auth）
```

- 動画は `Player` 内部のテクスチャに描画され、UI 層はそのテクスチャを背景として配置する分離設計。将来 UI バックエンドを差し替える際の負債軽減のため。
- mpv は `Box::leak` で `'static` 化し、`RenderContext<'static>` の自己参照を回避。
- すべての I/O（API 呼び出し・yt-dlp 解決・チャットポーリング）はバックグラウンドスレッドで実行し、結果は `mpsc::channel` でメインスレッドに送信される。
- egui のクロージャ内では `egui_glow` が `&mut self` として借用されているため、UI イベントは intent flag (`like_clicked` 等) を立てるだけにし、実際の処理は `egui_glow.run` の戻り後に実行。

詳細な設計判断（UI スタック選定の背景・配布形態への影響等）は [CLAUDE.md](CLAUDE.md) を参照。

## 必要環境

### macOS
```sh
brew install mpv yt-dlp
```
`build.rs` が `pkg-config --libs-only-L mpv` で libmpv のリンクパスを解決する。

### Windows
- Rust (MSVC toolchain) … `rustup default stable-x86_64-pc-windows-msvc`
- Visual Studio Build Tools 2022（VC Tools / Windows SDK）… libmpv のリンクに必要
- `tools/mpv-dev/`（libmpv 開発パッケージ）, `tools/yt-dlp.exe`

`mpv.lib`（MSVC 用インポートライブラリ）は `libmpv-2.dll` のエクスポートから生成済み。再生成は vcvars 環境で:
```powershell
dumpbin /exports libmpv-2.dll   # mpv_ で始まる関数名を mpv.def の EXPORTS に列挙
lib /def:mpv.def /name:libmpv-2.dll /out:mpv.lib /machine:x64
```

## ビルド

### macOS
```sh
cargo build               # debug
cargo build --release     # release
```

### Windows
```powershell
.\build.ps1            # debug
.\build.ps1 -Release   # release
```
ビルド後、実行に必要な `libmpv-2.dll` と `yt-dlp.exe` が `target\debug`（または `release`）にコピーされる。

## ログインの設定（高評価・登録チャンネル・再生リストに必要）

**client_secret は配布アプリには持たせない。** 認証の同意画面のみブラウザに委譲し、secret が必要なトークン交換は **Cloudflare Worker**（[auth-worker/](auth-worker/)）が代行する。アプリは「Worker の URL」だけを知り、トークン取得後は Data API を直接呼ぶ。

```
[アプリ] ─ブラウザで同意→ループバックでcode ─▶ [Worker] ─(id+secret付与)→ Google
        ◀──────── access / refresh token ───────
        ── Data API を直接 ──────────────────▶ YouTube API
```

### 1. Google 側
1. [Google Cloud Console](https://console.cloud.google.com/) でプロジェクト作成。
2. **YouTube Data API v3** を有効化。
3. **OAuth 同意画面**（User Type: 外部）。テストユーザーに自分のアカウントを追加。スコープ `.../auth/youtube.force-ssl`。
4. **OAuth クライアント ID** を作成。種類は **デスクトップ アプリ**（`127.0.0.1` への戻りが許可される）。client_id と client_secret を控える。

### 2. Worker のデプロイ（[auth-worker/](auth-worker/)）
`wrangler`（`npm i -g wrangler`）と Rust + `wasm32-unknown-unknown` ターゲットが必要。
```bash
cd auth-worker
# wrangler.jsonc の vars GAUTH_CLIENT_ID を自分の client_id に書き換える
wrangler secret put GAUTH_CLIENT_SECRET    # client_secret を貼り付け（配布物には入らない）
wrangler deploy                            # → https://<worker-name>.<account>.workers.dev
```

### 3. アプリ側
既定では本番 Worker（`https://youtube-super-lite-backend.cancer6.workers.dev`）に接続する。ローカル `wrangler dev` 等で別の Worker を使う場合のみ `--debug-backend` で上書きする。

```sh
./target/debug/youtube-super-lite                                          # 本番Worker
./target/debug/youtube-super-lite --debug-backend http://127.0.0.1:8787    # ローカル開発
```

- リフレッシュトークンは次のパスに保存され、次回は自動ログインを試みる:
  - macOS: `~/Library/Application Support/YouTubeSuperLite/auth.json`
  - Windows: `%APPDATA%\YouTubeSuperLite\auth.json`

## 実行

```sh
./target/debug/youtube-super-lite "https://www.youtube.com/watch?v=..."
```

引数で URL を渡すと起動時に再生。引数なしでも起動でき、URL 欄に貼り付けて Enter で再生できる。

### CLI オプション

```
youtube-super-lite [OPTIONS] [URL]
  -v, --verbose             mpv の詳細ログを出力（動作確認用）
      --debug-backend URL   認証バックエンドを上書き（デバッグ用、既定: 本番Worker）
      --enable-dev-tools    ローカル HTTP サーバを起動して検証用 API を公開（後述）
  -h, --help                ヘルプを表示
```

### 操作
- 動画は左側に表示。チャット表示中は動画を縮小して右側にチャットパネル（幅 320 dp）を並べる。
- ウィンドウ下部のコントロールで **再生/一時停止・シーク・音量** を操作。
- 上部に **🔑 YouTube ログイン** / **👍 高評価** / **💬 チャット表示・非表示** / **📋 おすすめ** / **📃 再生リスト** / **📺 新着** ボタン。
- キーボードショートカット（URL 欄に入力中は無効）:
  - `Space` 再生/一時停止
  - `←` / `→` 5秒シーク
  - `↑` / `↓` 音量 ±5
  - `Esc` 開いているオーバーレイ（おすすめ・再生リスト・新着）を閉じる
- 約3秒間マウス/キー操作がないと、URL欄・コントロール・マウスカーソルを自動的に隠す（動かすと再表示）。チャットパネルは恒常表示。

## 開発者向けツール (`--enable-dev-tools`)

`--enable-dev-tools` を付けて起動すると、`127.0.0.1` の OS 割当て ephemeral ポートで HTTP サーバが立ち上がり、検証用 API を公開する。フォーカス奪取や Accessibility 権限を必要とする `screencapture` / `cliclick` / `osascript` を経由せずに、アプリ自身が画面の状態を返せるようにすることが目的。

- バインドアドレスは **常に `127.0.0.1`**（ループバック固定。LAN からは到達不能）
- 起動時に stderr に `[dev-tools] http://127.0.0.1:NNNN` を出力するので、ポートはそこから取得する
- 認証なし。`--enable-dev-tools` を付けたプロセスを動かしているローカルユーザーが自身のために使うことを前提とする

### エンドポイント

| メソッド | パス | 返り値 | 説明 |
|----------|------|--------|------|
| `GET` | `/screenshot` | `image/png` | 現在の back buffer を PNG で返す（物理ピクセル解像度、HiDPI ではウィンドウサイズの 2 倍） |

スクショは egui の描画後・`swap_buffers` の直前に `glReadPixels` で取得するため、画面に映る内容（動画 + UI + ロード状態オーバーレイ）がそのままバイト列になる。動画未ロード時は中央に「動画を解決中…」「再生準備中…」「読み込み失敗 …」のいずれかが描画されるので、「真っ黒な画像」は実際に画面が黒い状態（つまりアプリの状態異常）を意味する。

### 使用例

```sh
./target/debug/youtube-super-lite --enable-dev-tools "https://www.youtube.com/watch?v=..." 2>/tmp/yt.stderr &
APP_PID=$!
sleep 3
PORT=$(grep -oE 'http://127.0.0.1:[0-9]+' /tmp/yt.stderr | head -1 | grep -oE '[0-9]+$')
curl -sS -o /tmp/shot.png "http://127.0.0.1:$PORT/screenshot"
kill $APP_PID
```

## DASH 対応の仕組み

ffmpeg（Homebrew 標準ビルド）は DASH demuxer 非対応のため、YouTube が DASH manifest しか返さないケース（終了ライブのアーカイブ等）は通常再生できない。本アプリでは:

1. `yt-dlp -g` でストリーム URL を取得
2. URL が DASH manifest（`manifest.googlevideo.com/api/manifest/dash/...`）と判定したら `dash-mpd` で MPD XML をパース
3. SegmentTemplate の `$Number$` 等を展開して各セグメントの URL を生成
4. mpv の EDL（`edl://!mp4_dash,init=...;seg1;seg2;...`）として組み立てて `loadfile` に渡す

これにより配信中ライブ・終了アーカイブ・短尺動画すべて再生できる。

## 制限事項・今後

- 配布形態は未確定（現状はソースビルド前提）。リリース時は LGPL ビルドの mpv 同梱と `.app` バンドル化が必要（詳細は [CLAUDE.md](CLAUDE.md)）。
- 高評価の現在状態表示・トグル（`videos.getRating`）未実装。
- 検索機能未実装。
- フルスクリーン切替未実装。
- カスタム絵文字のうち画像 URL のみ対応。Author Badges（メンバー継続バッジ等）は未対応。
