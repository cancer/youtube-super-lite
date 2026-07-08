# p0-sabr-probe

issue #16（ログイン済みライブが SABR 化で再生不可）の**調査用 PoC プローブ**。
本体アプリ（`youtube-super-lite` / `ysl-core`）から**完全に独立**したクレートで、本体のコード・依存・実行時挙動には一切影響しない。

## 何を調べるか

7/4 に YouTube がログイン済み TV client のライブ応答を SABR 化し、`hlsManifestUrl` が消えて
`serverAbrStreamingUrl` + `adaptiveFormats` のみになった。SABR は `serverAbrStreamingUrl` に
protobuf（`VideoPlaybackAbrRequest`）を POST し UMP でメディアを受け取るプロトコル。
本プローブはその経路を最小構成で叩き、**メディアが返るか / 何が 403 の原因か**を実測する。

proto フィールド番号・UMP パート ID・リクエスト構成は [LuanRT/googlevideo](https://github.com/LuanRT/googlevideo) 準拠。
protobuf エンコード・UMP デコード・base64 は自前実装（`prost` 不使用）。nsig(n 変換)は同梱の `src/nsig.rs`（boa）。

## 使い方（要ログイン: `%APPDATA%\YouTubeSuperLite\auth.json` の refresh_token）

```sh
# 現行ライブ ID を取る
curl -sL https://www.youtube.com/@NASA/live | grep -oE 'v=[A-Za-z0-9_-]{11}'

# P0 + P0.5（PoToken 無し。Bearer だけ / n を nsig 変換して 403→200 反転するか）
cargo run -p p0-sabr-probe -- <liveVideoId>

# 案2 PoC（PoToken を束ねる）: node の PoToken 発行器が要る（下記）
cargo run -p p0-sabr-probe -- --potoken     <token.json> <liveVideoId>   # 匿名 WEB 経路
cargo run -p p0-sabr-probe -- --potoken-tv  <gen.mjs>     <liveVideoId>   # TVHTML5+Bearer 経路（session 一貫）
```

## PoToken 発行器（`node/`）

`--potoken*` モードは PoToken が要る。`node/` に LuanRT `bgutils-js` + `jsdom` を使う発行器を同梱。
本番方針は WebView2 での BotGuard 実行だが、PoC では「手段を問わず 1 個調達」でよいので Node を使う。

```sh
cd node && npm install    # bgutils-js jsdom youtubei.js googlevideo
node gen.mjs [visitorData]        # {"visitorData":..,"poToken":..} を stdout
node ref_sabr.mjs <id> [--pot|--web]   # 参照実装(@luanrt/googlevideo)で同一経路を叩く検証用
```

## 結論（詳細は issue #16 のコメント）

- **PoToken は不要**。`serverAbrStreamingUrl` の `n=` を nsig 変換して POST すれば、Bearer だけで
  実メディア（UMP の MEDIA パート）が返る。`pot` は URL 署名(sparams)対象外で、束ねても挙動不変
  ＝ PoToken は真因でない（IP バインドも `ip=` 一致で否定）。
- **必須なのは正しい `n`（nsig 変換）**。この SABR エンドポイントは不正 `n` を（media GET の
  スロットルと違い）403 にする。app 同梱の boa nsig（`src/nsig.rs`）で end-to-end に ~1MB/req のメディア受信を確認。
- **挙動は間欠的**（YouTube の段階ロールアウト/実験バケット）。同一コードでも時間帯により
  `403`（参照実装でも 403）↔ `メディア受信` が入れ替わる。**アプリ側の解決経路は決定的でランダム性なし**。
- **go/no-go: 案1(SABR = Rust ネイティブ)は実現可能**。PoToken/WebView2/BotGuard（案2）は不要。
  protobuf/UMP/nsig はすべて Rust で完結。間欠性は「HLS が返れば優先・無ければ SABR・リトライ/堅牢化」で吸収する。
