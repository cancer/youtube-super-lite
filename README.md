# YouTube Super Lite

Rust 製の YouTube プレーヤー（**Windows**。macOS は将来対応予定）。

- 再生エンジン: **libmpv (mpv)** を `wid`（ウィンドウハンドル）に埋め込み、`vo=gpu-next` `gpu-api=d3d11` で
  **mpv 自身が D3D11 にウィンドウへ直接描画**する（OpenGL は一切使わない）
- ウィンドウ / イベントループ: **winit**（生成したウィンドウの HWND を mpv の `wid` に渡す）
- 操作 UI: **子窓 + DirectComposition** による透過オーバーレイ
- YouTube URL の解決: アプリ内蔵の**ネイティブ InnerTube リゾルバ**（yt-dlp は同梱・使用していない）

機能・設計の詳細は **[docs/](docs/README.md)** にまとめてある。今後の計画・未着手メモは [inbox/](inbox/) を参照。

## 必要環境（Windows）

- Rust (MSVC toolchain) … `rustup default stable-x86_64-pc-windows-msvc`
- Visual Studio Build Tools 2022（VC Tools / Windows SDK）… libmpv のリンクに必要
- `tools/mpv-dev/`（libmpv 開発パッケージ）

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

Cargo ワークスペースとして本体（`youtube-super-lite`）と `resolver-sidecar`（URL解決フォールバック用）を
同じ `target\<debug|release>` にビルドする。ビルド後、実行に必要な `libmpv-2.dll` が
`target\debug`（または `release`）にコピーされる。配布バンドル作成は `bundle.ps1` を使う。

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
      --enable-dev-tools    検証用ローカル HTTP を有効化（[docs/features/devtools.md](docs/features/devtools.md)）
  -h, --help                ヘルプを表示
```

## ログインの設定

高評価・登録チャンネル・再生リスト・履歴・視聴履歴記録を使うには OAuth ログインの設定が必要。
手順は [docs/features/login-and-rating.md](docs/features/login-and-rating.md) を参照。

## もっと詳しく

- **[docs/](docs/README.md)** — 機能軸・設計軸のドキュメント集（本README はこの入口）
- [inbox/](inbox/) — 今後の計画・未着手メモ
