# ドキュメント集

このディレクトリは YouTube Super Lite の現状ドキュメントを **機能軸**（何ができるか）と
**設計軸**（なぜそう作ったか）に分けてまとめたもの。今後の計画・未着手メモは
[../inbox/](../inbox/) を参照。

## 機能軸（[features/](features/)）— 何ができるか

| ドキュメント | 内容 |
|------|------|
| [playback.md](features/playback.md) | 動画再生、画質/コーデック切替、シーク・キャッシュ挙動 |
| [controller-ui.md](features/controller-ui.md) | コントローラ帯・キーボード操作・自動非表示 |
| [chat.md](features/chat.md) | ライブチャット（配信中/アーカイブ両対応） |
| [browse-lists.md](features/browse-lists.md) | 一覧オーバーレイ（おすすめ/登録新着/履歴/再生リスト） |
| [playlists.md](features/playlists.md) | 再生リストの一覧・中身表示 |
| [login-and-rating.md](features/login-and-rating.md) | ログイン設定手順・高評価 |
| [watch-history-tracking.md](features/watch-history-tracking.md) | YouTube側への視聴履歴反映（mark_watched） |
| [ui-settings-and-gpu.md](features/ui-settings-and-gpu.md) | UI設定の永続化・画像キャッシュ・GPU負荷監視 |
| [devtools.md](features/devtools.md) | 検証用ローカル HTTP（`--enable-dev-tools`） |

## セットアップ手順（[setup/](setup/)）

| ドキュメント | 内容 |
|------|------|
| [oauth-setup.md](setup/oauth-setup.md) | OAuthログイン機能を使うための Google Cloud Console / Cloudflare Worker のセットアップ手順 |

## 設計軸（[design/](design/)）— なぜそう作ったか

| ドキュメント | 内容 |
|------|------|
| [design-principles.md](design/design-principles.md) | 何を実装するときも従う設計原則(データ/振る舞い分離・情報隠蔽・明示的依存・ID参照) |
| [architecture-overview.md](design/architecture-overview.md) | 全体のモジュールマップと設計原則 |
| [url-resolution.md](design/url-resolution.md) | ネイティブ InnerTube リゾルバ・nsig・sidecar フォールバック |
| [overlay-rendering.md](design/overlay-rendering.md) | 子窓+DirectComposition オーバーレイと旧方式からの移行 |
| [auth-backend.md](design/auth-backend.md) | OAuth + Cloudflare Worker によるトークン交換の分離 |
| [threading-and-io.md](design/threading-and-io.md) | バックグラウンドスレッド + mpsc + `EventLoopProxy` のパターン |
| [dash-playback.md](design/dash-playback.md) | DASH manifest を mpv EDL に変換して再生する仕組み |

## 読み方の目安

- 「これは何をする機能か / どう操作するか」を知りたい → **features/**
- 「なぜこの実装になっているか / 何を置き換えたか」を知りたい → **design/**
- ビルド方法・環境構築・CLIオプションの一覧は [../README.md](../README.md) を参照
