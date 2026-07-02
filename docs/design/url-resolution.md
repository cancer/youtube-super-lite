# URL解決の設計（ネイティブ InnerTube リゾルバ）

対象読者: 再生できない/遅い/403 になるなど「そもそも再生 URL がどう決まるか」を追う人。

## なぜ自前で解決するか

ffmpeg・mpv 標準の `ytdl_hook` は yt-dlp 経由で解決するが、YouTube の変化への追従や DASH manifest 対応、
既存 UI（画質/コーデック選択、GPU負荷に応じた自動フォールバック）との統合の都合上、
このプロジェクトは **アプリ自身が InnerTube を直接叩いて解決する**方式を採る（`resolve/` 以下）。

過去は `yt-dlp.exe` をURLごとに `Command::spawn` していたが、1 回あたり数秒かかりレイテンシが大きかった。
現在は常駐ワーカースレッドが `reqwest::blocking::Client` と訪問者セッション Cookie を使い回すことで、
プロセス起動コストを毎回払わずに済む設計に置き換えられている。**yt-dlp.exe は配布物・依存関係から
撤去済み**（旧バージョンでは同梱していたが、ネイティブリゾルバ移行後に削除）。

## 構成要素

| ファイル | 役割 |
|------|------|
| `resolve/mod.rs` | 常駐ワーカーのメインループ。`ResolveRequest` を受けて `ResolveUpdate` を返す。フォーマット選択・sidecar フォールバックの調停もここ |
| `resolve/clients.rs` | InnerTube クライアント定義（ANDROID_VR / ANDROID / TVHTML5 のUA・バージョン・デバイス情報JSON）、動画ID抽出、訪問者データ取得 |
| `resolve/nsig.rs` | `n` パラメータの署名解除（nsig）。boa（Rust製JSエンジン）で YouTube の player base.js を実行する |

## クライアントの使い分け

| ケース | 使用クライアント | 理由 |
|------|------|------|
| 匿名 VOD | **ANDROID_VR** | 署名解除（nsig）が不要で、2160p までの adaptive フォーマットを取得できる |
| 匿名ライブ | **ANDROID** | HLS マニフェストを返す |
| メンバー限定・年齢制限などログインが要る動画 | **TVHTML5 + OAuth Bearer** | ログインを要求する動画で使える数少ないクライアント。ただし返る URL に `n` パラメータの署名解除が必要 |

判定・切り替えは `resolve/mod.rs` の解決フローが行い、呼び出し側（`Controller`）はクライアント種別を
意識しない。

## nsig（署名解除）の仕組み

TVHTML5 経由で取得した URL はそのままだとサーバ側スロットリングで再生できない（`n` パラメータの変換が必要）。

1. `https://www.youtube.com/iframe_api` を取得し、現在の player バージョン（ハッシュ、例: `445213fb`）を抽出
2. `https://www.youtube.com/s/player/<hash>/player_ias.vflset/en_US/base.js` を取得
3. base.js 内から `n` 変換関数（`$o6` のような難読化名）を正規表現で検出し、IIFE の終端を見つけて
   `window.__ytnsig=<関数>` のエクスポートと descramble ラッパーを注入
4. `window` / `document` / `navigator` / タイマー等の最小限のスタブとともに **boa** の JS コンテキストへロード
5. `descramble("n_value")` を呼び出して変換後の値を得る

player バージョンごとに結果をキャッシュするため、初回ロードのみ数秒（release ビルドで概ね 6.5 秒程度）かかり、
以降の変換は 1 回あたり ~17ms 程度。初回ロードのコストは、後述の sidecar 経路を並行して走らせることで
体感上隠蔽している。

**保守上の注意**: YouTube 側の player base.js の実装変更で、関数名検出の正規表現や IIFE 終端の検出が
追従を要することがある。再生開始直後に失敗するようになった場合、まずここを疑う。

## フォーマット選択

`adaptiveFormats`（映像/音声が分離されたストリーム）から、要求画質（`height` 以下で最も近いもの）・
コーデック（`avc1` / `vp09` / `av01` でフィルタ、Auto は最良を選択）に合うものを選ぶ。
`adaptiveFormats` が使えない場合は muxed の `formats` にフォールバックする。

## sidecar フォールバック

ネイティブ解決と並行して `resolver-sidecar.exe`（rustypipe ベースの別プロセス）も待機させる。
用途は 2 つ:

1. ネイティブ解決がボット判定等で失敗した場合、sidecar の結果を代わりに使う
2. mpv が再生開始後に 403 を返した場合（YouTube 側のURL失効等）、sidecar 側の結果に切り替えて再試行する

同時に有効な sidecar プロセスは 1 つ。sidecar には要求された画質・コーデックがそのまま引数として渡される。

## 関連

- [../features/playback.md](../features/playback.md) — 画質/コーデック切替のユーザー向け挙動
- [dash-playback.md](dash-playback.md) — DASH manifest 対応との接続点
- [inbox/native-resolver-spec.md](../../inbox/native-resolver-spec.md) — 元の設計メモ（M1-M16 等のマイルストーン）
