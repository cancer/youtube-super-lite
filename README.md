# Talava Player

Rust 製の YouTube プレーヤー。

- 再生エンジン: **libmpv (mpv)** の Render API（OpenGL）
- ウィンドウ / GL コンテキスト: **winit** + **glutin**
- 操作 UI: **egui**（動画の上に URL 入力欄と再生コントロールを重ねて表示）
- YouTube URL の解決: **yt-dlp**（mpv の ytdl_hook 経由）

mpv が動画を OpenGL の既定フレームバッファに描画し、その上へ egui の UI を合成する。
これにより再生コントロールやプレイリストなどの UI を将来重ねていける。

## 構成

| ファイル | 役割 |
|----------|------|
| [src/main.rs](src/main.rs) | GL コンテキスト生成 → mpv Render API で動画描画 → egui で UI を重ねる |
| [src/auth.rs](src/auth.rs) | YouTube OAuth2（ループバック）＋ Data API（高評価・チャンネル名）。トークン交換は Worker に委譲 |
| [auth-worker/](auth-worker/) | Cloudflare Worker（workers-rs）。client_secret を保持しトークン交換を中継 |
| [build.rs](build.rs) | `tools/mpv-dev` をリンク検索パスに追加（libmpv2-sys は検索パスを設定しないため） |
| [build.ps1](build.ps1) | MSVC 環境(vcvars) + `MPV_SOURCE` を設定して `cargo build`、実行用 DLL/exe をコピー |
| `tools/mpv-dev/` | libmpv 開発ファイル（`libmpv-2.dll` / ヘッダ / 生成した `mpv.lib`） |
| `tools/yt-dlp.exe` | YouTube ストリーム解決用 |

> 注: 描画合成のため再生エンジンは `libmpv2` **6.x** を使用（4.x には Render API 利用時に
> use-after-free でクラッシュするバグがある）。

## 必要環境（Windows）

- Rust (MSVC toolchain) … `rustup default stable-x86_64-pc-windows-msvc`
- Visual Studio Build Tools 2022（VC Tools / Windows SDK）… libmpv のリンクに必要
- `tools/mpv-dev/`（libmpv 開発パッケージ）, `tools/yt-dlp.exe`

`mpv.lib`（MSVC 用インポートライブラリ）は `libmpv-2.dll` のエクスポートから生成済み。
再生成する場合は vcvars 環境で:

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

## ログインの設定（高評価に必要）

**client_secret は配布アプリには持たせない。** 認証の同意画面のみブラウザに委譲し、
secret が必要なトークン交換は **Cloudflare Worker**（[auth-worker/](auth-worker/)）が代行する。
アプリは「Worker の URL」だけを知り、トークン取得後は Data API を直接呼ぶ。

```
[アプリ] ─ブラウザで同意→ループバックでcode ─▶ [Worker] ─(id+secret付与)→ Google
        ◀──────── access / refresh token ───────
        ── videos.rate を直接 ─────────────────▶ YouTube API
```

### 1. Google 側

1. [Google Cloud Console](https://console.cloud.google.com/) でプロジェクト作成。
2. **YouTube Data API v3** を有効化。
3. **OAuth 同意画面**（User Type: 外部）。テストユーザーに自分のアカウントを追加。スコープ `.../auth/youtube.force-ssl`。
4. **OAuth クライアント ID** を作成。種類は **デスクトップ アプリ**（`127.0.0.1` への戻りが許可される）。client_id と client_secret を控える。

### 2. Worker のデプロイ（[auth-worker/](auth-worker/)）

`wrangler`（`npm i -g wrangler`）が必要。Rust + `wasm32-unknown-unknown` ターゲットも要る。

```bash
cd auth-worker
# wrangler.jsonc の vars GAUTH_CLIENT_ID を自分の client_id に書き換える
wrangler secret put GAUTH_CLIENT_SECRET    # client_secret を貼り付け（配布物には入らない）
wrangler deploy                            # → https://<worker-name>.<account>.workers.dev
```

### 3. アプリ側

環境変数で Worker の URL を指定して起動する（既定は `http://127.0.0.1:8787`＝`wrangler dev` 用）。

```powershell
$env:TALAVA_AUTH_BACKEND = "https://talava-auth.<account>.workers.dev"
.\target\debug\talava-player.exe
```

上部の **🔑 YouTube にログイン** → ブラウザで承認 → 戻ると **👍 高評価** が使える。

- リフレッシュトークンは `%APPDATA%\TalavaPlayer\auth.json` に保存され、次回は自動ログインを試みる。
- 配布物・リポジトリに secret は含まれない（secret は Worker の Secret のみ）。`auth.json` は `.gitignore` 済み。

## 実行

```powershell
.\target\debug\talava-player.exe "https://www.youtube.com/watch?v=..."
```

- 引数で URL を渡すと起動時に再生。引数なしでも起動でき、**ウィンドウ上部の URL 欄に貼り付けて Enter** で再生できる。
- 読み込んだ動画の **タイトル** を表示。
- **YouTube ログイン**（OAuth2）と **👍 高評価**（YouTube Data API `videos.rate`）をアプリ内で実行。
  認証（同意画面）のみブラウザに委譲し、それ以外の操作は API で行う。設定は「ログインの設定」を参照。
- ウィンドウ下部のコントロールで **再生/一時停止・シーク・音量** を操作できる。
- キーボードショートカット（URL 欄に入力中は無効）:
  - `Space` 再生/一時停止
  - `←` / `→` 5秒シーク
  - `↑` / `↓` 音量 ±5
- 約3秒間マウス/キー操作がないと、URL欄・コントロール・マウスカーソルを自動的に隠す（動かすと再表示）。
- `TALAVA_VERBOSE=1` を設定すると mpv の再生ステータスとフレーム数を端末に出力（動作確認用）。

```powershell
$env:TALAVA_VERBOSE="1"; .\target\debug\talava-player.exe "https://youtu.be/..."
```

## 今後

- 高評価の現在状態表示（`videos.getRating`）・トグル、登録チャンネル等
- プレイリスト / 検索
- フルスクリーン切替
