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
| 匿名VODのフォールバック | **ANDROID** | ANDROID_VR が失敗した場合の予備 |
| 匿名ライブ | **ANDROID**（試みるが現状ほぼ失敗） | 後述の「ライブの bot ゲート」を参照。実質 sidecar 頼みになっている |
| ログイン中のライブ | 未実装（ネイティブ側に専用経路なし） | 後述参照 |
| メンバー限定・年齢制限などログインが要る動画（gated） | **`resolver-sidecar.exe`（rustypipe）へ全面委譲** | 後述の理由でネイティブ解決の TVHTML5 経路は使っていない |

判定・切り替えは `resolve/mod.rs` の解決フローが行い、呼び出し側（`Controller`）はクライアント種別を
意識しない。

## ライブの bot ゲート（既知の制限）

2026-07 時点で、YouTube は**ライブ配信を匿名の InnerTube クライアント（web/tv/ios/android/android_vr/mweb 等）
全般に対して bot ゲート**するようになっている（`playabilityStatus=LOGIN_REQUIRED`、`streamingData` なし）。
このため、上表の「匿名ライブ → ANDROID」は実装としては存在するものの、**実際にはほぼ常に失敗し**、
`resolve/mod.rs` のフォールバックにより sidecar 経由の解決に委ねられているのが実態。

ログイン中であれば TVHTML5 + OAuth Bearer で `hlsManifestUrl` を取得する経路が有効（HLS はセグメント
取得に nsig 変換が不要なため、VOD で述べた「nsig未接続」の制約はライブには当てはまらない）だが、
**このネイティブリゾルバの現行実装にはまだこの経路が組み込まれていない**。未ログイン時のライブ解決は
PoToken/botguard 相当の対応が別途必要で、依然として未解決。

## nsig（署名解除）: 実装はあるが現在は未使用（dead code）

**重要**: `resolve/nsig.rs` に nsig 変換の実装（boa エンジンで base.js を実行する仕組み）自体は存在するが、
`resolve_one()`（`resolve/mod.rs`）の実際の解決フローからは**呼ばれていない**。TVHTML5 + OAuth Bearer で
gated（メンバー限定・年齢制限）動画を解決しようとするコードパス自体が実装されておらず、常に
「ネイティブ解決不可」としてエラーを返し、後述の **sidecar フォールバックへ処理を委譲する**設計になっている
（`resolve/mod.rs` 内のコメント参照）。

未使用になっている理由:

1. **署名(`s`)復号が未実装** — TVHTML5 が返す一部フォーマットは `n` パラメータだけでなく `s`
   パラメータ（従来の signatureCipher）の復号も必要だが、この処理自体を実装していない
2. **nsig抽出が現行 base.js で破綻** — 現在配信されている base.js は VM 型難読化が強化されており、
   PoC 時点（`inbox/native-resolver-spec.md` の U5）で確認していた `$o6` のような単純な関数名検出では
   もはや対応関数を安定して切り出せない

`resolve/nsig.rs` の `NsigSolver` / `BoaEngine` はこの制約が解消された将来のために実装だけ残されている
（`req.access_token` も同様に「将来の認証経路用に温存」とコメントされている）。**現状の gated 動画再生は
100% sidecar 経由**であり、ネイティブ側のnsig変換が動いているわけではない。ドキュメント上も「nsigが
効いている」という前提で読まないこと。

nsig の変換ロジック自体の設計（実装済みだが未接続の部分の仕組み）:

1. `https://www.youtube.com/iframe_api` を取得し、現在の player バージョン（ハッシュ、例: `445213fb`）を抽出
2. `https://www.youtube.com/s/player/<hash>/player_ias.vflset/en_US/base.js` を取得
3. base.js 内から `n` 変換関数（`$o6` のような難読化名）を正規表現で検出し、IIFE の終端を見つけて
   `window.__ytnsig=<関数>` のエクスポートと descramble ラッパーを注入
4. `window` / `document` / `navigator` / タイマー等の最小限のスタブとともに **boa** の JS コンテキストへロード
5. `descramble("n_value")` を呼び出して変換後の値を得る

（設計上は player バージョンごとに結果をキャッシュし、初回ロードのみ数秒、以降は 1 回あたり ~17ms
程度になる想定だったが、上記の通り現在は呼び出されていないため実際の再生には影響しない。）

## フォーマット選択

`adaptiveFormats`（映像/音声が分離されたストリーム）から、要求画質（`height` 以下で最も近いもの）・
コーデック（`avc1` / `vp09` / `av01` でフィルタ、Auto は最良を選択）に合うものを選ぶ。
`adaptiveFormats` が使えない場合は muxed の `formats` にフォールバックする。

## sidecar（`resolver-sidecar.exe`）

ネイティブ解決と並行して `resolver-sidecar.exe`（rustypipe ベースの別プロセス）も常に起動して待機させる。
用途は 3 つ:

1. **メンバー限定・年齢制限などの gated 動画を解決する**（前述の通り、ネイティブ側にはこの経路の実装が
   ないため、gated 動画は実質 sidecar が唯一の解決手段）
2. ネイティブ解決（匿名 ANDROID_VR / ANDROID）がボット判定等で失敗した場合、sidecar の結果を代わりに使う
3. mpv が再生開始後に 403 を返した場合（YouTube 側のURL失効等）、sidecar 側の結果に切り替えて再試行する

同時に有効な sidecar プロセスは 1 つ。sidecar には要求された画質・コーデックがそのまま引数として渡される。

## 関連

- [../features/playback.md](../features/playback.md) — 画質/コーデック切替のユーザー向け挙動
- [dash-playback.md](dash-playback.md) — DASH manifest 対応との接続点
- [inbox/native-resolver-spec.md](../../inbox/native-resolver-spec.md) — 元の設計メモ（M1-M16 等のマイルストーン）
