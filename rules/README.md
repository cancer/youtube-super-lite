# declarativeNetRequest の静的ルール

要件 [R1](../inbox/chrome-extension-requirements.md) の実装。JSON にコメントを書けないため、
ルールの根拠と未確定事項をここに置く。

## なぜ用途別に分割するか

要件 §9-5 は「(b) を 1 本ずつ遮断して差分を取る」、§9-7 は「壊れるなら該当分だけ (c) に戻す」を求める。
`declarative_net_request.rule_resources` は ruleset 単位で `enabled` を持つので、
**個別に有効・無効を切れる粒度**が分割の単位になる。1 本の巨大な ruleset ではこの切り分けができない。

## `rules/` — 製品構成で有効

いずれも要件 §5 R1 (a)（純粋な計測・再生に関与しない）。

| ruleset | 対象 |
|---|---|
| `a-telemetry.json` | `/youtubei/v1/log_event`、`/api/stats/qoe`、`/ptracking` |
| `a-ads-tracking.json` | `/api/stats/ads`、`/api/stats/atr`、`/pagead/interaction/*`、`/pagead/viewthroughconversion/*` |
| `a-thumbnails.json` | `i.ytimg.com` のサムネイル（`/vi/`、`/vi_webp/`） |

`a-ads-tracking.json` を別ファイルにしてあるのは §9-7 のためである。計測ビーコンを落としたときに
広告の再生が止まる・本編へ復帰しないという副作用が出たら、**この ruleset だけを無効にして (c) に戻せる**。

### サムネイルをホスト一括で遮断しない理由

シークホバーのプレビュー（ストーリーボード）が同じ `i.ytimg.com` から配信される。
実際の URL 構成は次のように分かれている。

- サムネイル: `https://i.ytimg.com/vi/<videoId>/hqdefault.jpg`、`/vi_webp/<videoId>/mqdefault.webp`
- ストーリーボード: `https://i.ytimg.com/sb/<videoId>/storyboard3_L2/M0.jpg?sqp=...&sigh=...`
  （`i9.ytimg.com` など番号付きホストからも配信される）

ホスト一括で落とすと `/sb/` も巻き添えになり、R1 の受け入れ条件「シークが壊れない」に触れる。
そのため `/vi/` と `/vi_webp/` だけをパスで狙い、`/sb/` は対象外にした。
`tests/rules.test.ts` がストーリーボードの代表 URL を回帰テストとして固定している。

`/an_webp/`（サムネイルのホバーアニメーション）は本セッションで実在を確認できなかったため対象に入れていない。

### 副作用: 一覧ページのサムネイルも消える

静的ルールの `condition` は**リクエスト元ページのパスで絞れない**（`initiatorDomains` はホスト単位、
`tabIds` は動的ルール専用）。したがって `a-thumbnails.json` は watch ページに限定できず、
ホーム・登録チャンネル・履歴・再生リストのサムネイルも消える。
要件 §4.1 は「独立したページとしての一覧は残す」としているので、この副作用が許容できない場合は
この ruleset を `enabled: false` にする（他の遮断には影響しない）。

### 未確定: 広告の ID 同期系

要件 §5 R1 (a) は「ID 同期系」も対象に挙げているが、具体的なパスをこのセッションでは確定できなかった。
確証の無いパスを入れると (c)（広告の配信・表示に必要なリクエスト）を誤って落としうるので含めていない。
§6 の実測で対象を特定してから足す。

## `rules/experiments/` — 常に `enabled: false`

要件 §5 R1 (b) は「遮断してはならない」と明記している。ここにあるのは **§9-5 の切り分け専用**で、
製品構成では絶対に有効にしない。1 つだけ有効にして壊れる箇所を観測し、終わったら戻す。

| ruleset | 目的 |
|---|---|
| `exp-heartbeat.json` | `/youtubei/v1/player/heartbeat` を止めるとライブ再生の何が壊れるかを見る |
| `exp-attestation.json` | `/youtubei/v1/att/*` を止めると bot 判定通過の何が壊れるかを見る |
| `exp-prefetch.json` | 次動画のプリフェッチ / preconnect の削減効果を見る |

### 未確定: プリフェッチの識別手段

**次動画のメディアプリフェッチを、本再生の `videoplayback` と URL 上で区別する識別子は実測で確定していない。**
区別できないまま遮断すると本再生を壊すため、`videoplayback` を対象にするルールは書いていない。

`exp-prefetch.json` に入れてあるのは `googlevideo.com/generate_204` だけである。これは watch ページの
`<link rel="preload">` から発行される接続ウォームアップ（ノードの生存確認）で、メディア本体ではない。
`preconnect` はブラウザへのヒントであり、DNR で確実に落とせるとも限らない。

要件 §5 R1 は広告ビーコンについて「効果の期待値が低い項目」として §6 の実測を待てと述べている。
プリフェッチも同じ扱いにし、識別子が実測で確定するまで既定のルールには入れない。

`exp-prefetch.json` の `resourceTypes` は実測していない。`<link rel="preload">` 由来のリクエストが
どの種別で届くかを確認していないため、`ping` / `xmlhttprequest` / `other` を候補として並べてある。
この ruleset を有効にして観測するときは、まず種別が合っているかを確かめる。

## ルールを追加するときの制約

**ドメインアンカー `||` を使う `urlFilter` は、必ずホスト部の直後に `/` を置く。**
`||youtube.com` のようにホスト名で終わると、`www.youtube.com.example` のような別ホストにも一致する
（リファレンスが "incorrectly matches" と明言している挙動）。`tests/rules.test.ts` がこれを強制する。

## 検証の限界

`tests/rules.test.ts` が検査できるのは**ルール JSON の妥当性と、urlFilter が表す URL 集合**までである。
照合は `tests/support/url-filter.ts` による DNR 仕様の**模擬**であって、ブラウザの挙動の保証ではない。
実機での受け入れ確認（再生・シーク・ライブ継続・チャット受信・ログイン状態・視聴履歴の記録）は別に行う。
