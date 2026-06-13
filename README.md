# YouTube Super Lite

Rust 製の YouTube プレーヤー（**Windows**。macOS は将来対応予定）。

- 再生エンジン: **libmpv (mpv)** を `wid`（ウィンドウハンドル）に埋め込み、`vo=gpu-next` `gpu-api=d3d11` で
  **mpv 自身が D3D11 にウィンドウへ直接描画**する（OpenGL は一切使わない）
- ウィンドウ / イベントループ: **winit**（生成したウィンドウの HWND を mpv の `wid` に渡す）
- 操作 UI: **Direct2D + DirectWrite + WIC** による透過オーバーレイ（`WS_EX_LAYERED` 窓に描画し、
  `UpdateLayeredWindow(ULW_ALPHA)` で動画の上に per-pixel alpha 合成）
- YouTube URL の解決: **yt-dlp**（mpv の ytdl_hook ではなくアプリ側で直接呼ぶ）
- DASH manifest の処理: **`dash-mpd`** クレートでパースし mpv EDL に変換（ffmpeg に DASH demuxer がなくても再生可能）

> 設計の経緯: 以前は mpv の OpenGL Render API ＋ egui を単一 GL コンテキストで合成していたが、
> 起動時の OpenGL ドライバ bring-up が他アプリの GPU 再生を一瞬妨げる問題があり、
> 「mpv 埋め込み(D3D11) + Direct2D 2D UI」へ移行した（egui / glutin / glow は撤去済み）。
> 移行の記録は [inbox/opengl-to-native-migration.md](inbox/opengl-to-native-migration.md) を参照。

## 主な機能

| 機能 | 説明 | 認証 |
|------|------|------|
| 動画再生 | 短尺動画・配信中ライブ・終了ライブのアーカイブをすべて再生 | 不要 |
| コントローラ | 動画下部に再生/一時停止・シークバー・時間・画質/コーデック表示を Direct2D で重ねる。無操作で自動非表示 | 不要 |
| 高評価 | ログイン中ユーザーで `videos.rate` を呼んで高評価を付ける（Ctrl+G） | OAuth |
| ライブチャット | InnerTube `get_live_chat` 経由で取得し、動画を左に縮めて右側にチャットパネル表示（Ctrl+T） | 不要 |
| 一覧（おすすめ/登録新着/履歴/再生リスト） | 全画面の一覧オーバーレイ（Tab）。1/2/3/4 でソース切替、↑↓/クリックで選択、Enter で再生。サムネは WIC でデコード | 一部 OAuth |
| 再生リスト | Data API v3 `playlists.list?mine=true` → `playlistItems.list` を 2 階層（一覧→中身）で表示 | OAuth |
| 画像キャッシュ | サムネを自前取得して OS のキャッシュ領域に保存（URL→FNV ハッシュのファイル名）。WIC でデコードしてビットマップキャッシュ | 不要 |

## アーキテクチャ

```
[main.rs]                  — CLI 解析 → NativeApp 起動。共有の値型(Quality/Codec 等)と build_ytdlp_format を保持
[native_app::NativeApp]    — winit アプリ本体。HWND→mpv 埋め込み、キーボード/状態、各種 poll、Controller を駆動
[native_overlay::Overlay]  — 透過オーバーレイ。Direct2D/DirectWrite/WIC でコントローラ・URL欄・一覧・チャットを描画
[controller::Controller]   — UI 非依存のアプリ状態とロジック（mpv 制御・認証/API・yt-dlp 解決・各種 poll）
[player::Player]           — libmpv ラッパー。mpv を wid に D3D11 埋め込み（render API/OpenGL は使わない）
[image_cache]              — サムネのディスクキャッシュ（cache_dir/cached_path/ensure_cached_async）
[各機能モジュール]          — chat / recommend / subscriptions / playlist / history / resolve / auth / mark_watched / gpu_usage
```

- **描画の分離**: 動画は mpv が D3D11 で直接描く。UI は別の透過レイヤード窓に Direct2D で描き、OS コンポジタが重ねる
  （両者は GPU コンテキストを共有しない）。チャット表示時は mpv の `video-margin-ratio-right` で動画を左に縮め、
  空いた右側にチャットパネルを描く（真の左右分割）。
- **UI 非依存コア**: 状態とロジックは `Controller` に集約され、フロントエンド（描画/入力）から分離されている。
- すべての I/O（API 呼び出し・yt-dlp 解決・チャットポーリング・サムネ取得）はバックグラウンドスレッドで実行し、
  結果は `mpsc::channel` でメインスレッドへ送る。完了時は winit の `EventLoopProxy` でイベントループを起こす。

## 必要環境

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

```powershell
.\build.ps1            # debug
.\build.ps1 -Release   # release
```
ビルド後、実行に必要な `libmpv-2.dll` と `yt-dlp.exe` が `target\debug`（または `release`）にコピーされる。

## ログインの設定（高評価・登録チャンネル・再生リスト・履歴に必要）

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
既定では本番 Worker（`https://youtube-super-lite-backend.cancer6.workers.dev`）に接続する。ローカル `wrangler dev` 等で別の Worker を使う場合のみ `--debug-backend` で上書きする。アプリ内では **Ctrl+L** でログインを開始する。

- リフレッシュトークンは次のパスに保存され、次回は自動ログインを試みる:
  - Windows: `%APPDATA%\YouTubeSuperLite\auth.json`

## 実行

```powershell
.\target\debug\youtube-super-lite.exe "https://www.youtube.com/watch?v=..."
```

引数で URL を渡すと起動時に再生。引数なしでも起動でき、英数字キーで URL を入力（または Ctrl+V で貼り付け）して Enter で再生できる。

### CLI オプション

```
youtube-super-lite [OPTIONS] [URL]
  -v, --verbose             mpv の詳細ログを出力（動作確認用）
      --debug-backend URL   認証バックエンドを上書き（デバッグ用、既定: 本番Worker）
      --volume N            初期音量 0-130（デバッグ用。例: --volume 0 で無音）
      --enable-dev-tools    検証用ローカル HTTP を有効化（後述の dev-tools）
  -h, --help                ヘルプを表示
```

### dev-tools（`--enable-dev-tools`）

外部の screencapture / クリックツールに依存せず、アプリ自身がローカル HTTP で
スクリーンショット撮影・UI 操作注入を受け付ける検証用サーバ。起動時に listen ポートを
stderr に表示する（`[dev-tools] http://127.0.0.1:<port> ...`）。`curl` だけで検証フローを回せる。

| メソッド / パス | 説明 |
|------|------|
| `GET /screenshot` | 現在のウィンドウ（クライアント領域）を PNG で返す。撮影前にウィンドウを前面化し、オーバーレイ込みの合成画を取得する |
| `POST /action/<name>` | UI 操作を起こす。`<name>`: `toggle_chat` / `play_pause` / `login` / `like` / `close_overlay` / `open_recommend` / `open_subs` / `open_playlist` / `open_history` |
| `POST /click?x=&y=` | クライアント px 座標に左クリックを注入（コントロール矩形へ振り分け） |
| `POST /type`（body=text, `?enter=1`） | URL 欄へテキスト入力。`enter=1` で再生 |

### 操作（キーボード中心）

- 動画は全画面。下部のコントローラ（Direct2D）に再生/一時停止・シークバー・時間・画質/コーデックを表示。
- 一覧・チャットは透過オーバーレイで重ねる（チャットは動画を左に縮めて右側に表示）。

| キー | 動作 |
|------|------|
| 英数字 / 記号 | URL 欄へ入力（URL は空白を含まないため Space は再生に使える） |
| `Ctrl`+`V` | クリップボードの URL を貼り付け |
| `Enter` | URL 欄の URL を再生 |
| `Space` | 再生 / 一時停止 |
| `←` / `→` | 5 秒シーク |
| `↑` / `↓` | 音量 ±5 |
| `Tab` | 一覧オーバーレイの開閉 |
| 一覧中 `1`/`2`/`3`/`4` | ソース切替（登録新着 / おすすめ / 履歴 / 再生リスト） |
| 一覧中 `↑`/`↓`・クリック | 項目選択 |
| 一覧中 `Enter` | 選択を再生（再生リストは中身を開く） |
| 一覧中 `Backspace` | 再生リストの中身から一覧へ戻る |
| `Ctrl`+`T` | チャットの開閉 |
| `Ctrl`+`L` | ログイン開始 |
| `Ctrl`+`G` | 現在の動画に高評価 |
| `Ctrl`+`Q` / `Ctrl`+`C` | 画質 / コーデックの切替（再生中なら取り直す） |
| `Ctrl`+`-` / `Ctrl`+`+` | コメント（チャット）の文字サイズを縮小 / 拡大 |
| `Esc` | 一覧を閉じる / URL 欄をクリア |

- 約 3 秒間マウス/キー操作がないと、オーバーレイ（コントローラ・URL 欄）を自動的に隠す（動かすと再表示）。
  一覧・チャット表示中は隠さない。

## DASH 対応の仕組み

ffmpeg（標準ビルド）は DASH demuxer 非対応のため、YouTube が DASH manifest しか返さないケース（終了ライブのアーカイブ等）は通常再生できない。本アプリでは:

1. `yt-dlp -g` でストリーム URL を取得
2. URL が DASH manifest（`manifest.googlevideo.com/api/manifest/dash/...`）と判定したら `dash-mpd` で MPD XML をパース
3. SegmentTemplate の `$Number$` 等を展開して各セグメントの URL を生成
4. mpv の EDL（`edl://!mp4_dash,init=...;seg1;seg2;...`）として組み立てて `loadfile` に渡す

これにより配信中ライブ・終了アーカイブ・短尺動画すべて再生できる。

## 制限事項・今後

計画・未着手メモは [inbox/](inbox/) に置く。

- **macOS 未対応**（D3D11 + Direct2D は Windows 前提。Metal + CoreAnimation への対応が今後の課題）。
- 登録チャンネルは新着フィードのみ対応（特定チャンネルのアップロード一覧へのドリルダウンは未移植）。
- 高評価の現在状態表示・トグル（`videos.getRating`）未実装。
- 検索機能未実装（日本語 IME も未対応。現状の入力は ASCII の URL のみ）。
- フルスクリーン切替未実装。
- カスタム絵文字（チャットのメンバーシップスタンプ等）は画像をインライン描画する（未デコード時のみ alt テキスト）。
- 配布形態は未確定（現状はソースビルド前提）。
