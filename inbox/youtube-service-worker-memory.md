# 調査: YouTube の service worker のメモリを削れるか

対象読者: この拡張を実装する人。結論だけ要るなら §1 と §3 を読む。

要件 [§2](chrome-extension-requirements.md) の評価軸の最優先は「長時間視聴での定常メモリ増加」である。
この文書は、その増加源として名指しされた **YouTube 自身の service worker**（`https://www.youtube.com/sw.js`、
scope `/`）を拡張から削れるかを調べた記録である。

## 0. この調査の確度

**本セッションでは実機を観測していない。** 作業環境の egress ポリシーが `www.youtube.com` への接続を
403 で拒否するため、`sw.js` の中身も、Chrome のメモリ実測も取れていない
（`connect_rejected: www.youtube.com:443`）。

したがって以下は **①ブラウザの機構から導ける確実な部分**と、**②実機で潰すべき手順**の 2 つに分けて書く。
数値と「本当に効くか」は §5 を実行してから埋める。§5 を飛ばして §3 を実装しないこと。
この repo が [rules/README.md](../rules/README.md) で「確証の無いパスは入れない」としているのと同じ扱いにする。

## 1. 結論（先に）

- **拡張から消せる。** 手段は「登録そのものを消す + 再登録させない」の 2 つ 1 組で、追加権限ゼロの経路がある。
- **消す価値がある可能性は高いが、未実測。** 「視聴中の SW は一度も停止しない」という機構上の理由
  （§2）が、最優先評価軸の症状と辻褄が合う。ただし辻褄が合うことは証拠ではない。
- **代償は YouTube の Web Push 通知（デスクトップ通知）が来なくなること。** 要否はユーザーの判断。
- **副産物として R1 が確実になる。** service worker が応答を作った場合や CacheStorage から返した場合、
  declarativeNetRequest はその応答に効かない。SW が居なくなればこの穴自体が消える。

## 2. なぜ増え続けるのか（機構からの推論）

Chrome のタスクマネージャの `Service worker: https://www.youtube.com/` は、YouTube が登録した
service worker のグローバルスコープを載せる renderer プロセスの計上である。
**この拡張自身の service worker（`src/background/sw.ts`）とは無関係**なので混同しないこと。

増加の筋書きは次のとおり。

1. **制御下のページが出すリクエストは、クロスオリジンのサブリソースも含めて全部 `fetch` イベントになる。**
   watch ページで言えば `googlevideo.com` のメディアセグメント、`get_live_chat` のポーリング、
   `stats` 系のビーコンが、数秒おきに SW のスレッドへ届き続ける。
2. **Chrome は idle 約 30 秒で SW を止めるが、1 のせいで idle にならない。**
   3 時間の配信を見れば SW は 3 時間生き続ける。プロセスが終わらない以上、
   ピークまで膨らんだフットプリントが OS へ返る機会も無い。
3. Chromium 側にも
   [ServiceWorker memory footprint consistently increasing (issue 40260507)](https://issues.chromium.org/issues/40260507)
   が立っている。

**要確認:** 1 と 2 は「SW が `fetch` ハンドラを持っている」ことが前提である。Chrome は fetch ハンドラの無い
登録ではリクエスト時に SW を起動しない。YouTube の `sw.js` が fetch ハンドラを持つかは未確認で、
持たない場合はこの筋書きは崩れ、犯人は別（push / periodic sync など）になる。§5-1 で最初に潰す。

## 3. 拡張から取れる手段

| 手段 | 効果 | 追加権限 | 代償 |
|---|---|---|---|
| A. 登録ごと消す | SW プロセスが消える（最大） | 0 〜 `browsingData` | Web Push 通知 |
| B. 生かしたまま定期的に落とす | ヒープが周期的に戻る | `browsingData` | 視聴中に落として壊れないか未検証 |
| C. 触らずイベントを減らす（= 現行の R1/R2/R3） | 増加が遅くなるだけ | 0 | — |

**C 単独では解決にならない。** メディアセグメントの取得が続く限り SW は idle に落ちないので、
リクエストを減らしても SW の寿命は縮まない。

### A. 登録ごと消す（推奨）

2 つで 1 組。**片方だけでは効かない。**

**A-1. 既に入っている登録を消す**

- 追加権限ゼロの経路: MAIN world / `document_start` の content script で
  `navigator.serviceWorker.getRegistrations()` → 各 `unregister()`。
  ただし `unregister()` は**既存クライアントの制御を続けたまま**で、そのタブの worker は死なない。
  無制御になるのは次のロードから。「開いているタブには効かない」ことを許容できるならこれで足りる。
- 今動いている worker ごと落とす経路:
  `chrome.browsingData.remove({ origins: ["https://www.youtube.com"] }, { serviceWorkers: true })`。
  即座に効く代わりに `browsingData` 権限が要り、インストール時の警告文が 1 行増える。

**A-2. 再登録させない**

- DNR 静的ルールで `||youtube.com/sw.js` を block する。
  **既にインストール済みの登録はスクリプトを取りに行かないので、これ単独では現状のメモリは減らない**
  （効くのは更新チェックと新規登録だけ）。A-1 と必ず対で使う。
  `resourceTypes` は未実測（`script` / `other` が候補）。[rules/README.md](../rules/README.md) の
  `exp-prefetch` と同じ扱いで、まず種別が合っているかを確かめる。
- 併せて MAIN world で `ServiceWorkerContainer.prototype.register` を reject に差し替えると、
  DNR の種別ミスに依存しなくなり、ページには「登録に失敗した」という既知の経路で返る。

**スコープの注意:** content script は `/watch*` と `/live_chat*` にしか入らない。ホームや検索結果から
入った場合はそこで再登録される。origin 全体に効くのは DNR だけなので、**DNR が主・content script が補助**
という組み方になる。要件 §3 のスコープ（watch ページ）を広げずに済ませるための配分である。

**失うもの:** YouTube の Web Push 通知。SW 経由のナビゲーション高速化（体感差は未計測）。
視聴履歴（`api/stats/playback` / `watchtime`）はページから直接飛ぶので影響しないはずだが §5-5 で確認する。

### B. 生かしたまま定期的に落とす

通知を残したい場合の代案。`browsingData.remove` を周期的に、あるいはタブを閉じたときに呼ぶ。
実行中の worker ごと落ちるのでヒープは戻る。**視聴中に落として再生・チャットが壊れないかは未検証。**

拡張から `ServiceWorker.stopWorker`（worker を殺すだけで登録は残す）を呼ぶには `chrome.debugger` が要る。
常時「デバッグ中」のバナーが出るので採らない。

## 4. 要件との整合

- **§2 の目的そのもの。** 広告の表示には関与しない（SW は広告配信の経路ではない。§5-5 で確認）ので、
  §2 の「広告の回避は意図しない」に触れない。
- **§3 のスコープ:** DNR は origin 全体に効くため watch 以外のページにも影響する。
  `a-thumbnails.json` と同じ性質の副作用なので、同じ作法で **独立した ruleset として切れるようにする**。
- **R1 との関係:** SW が居る間、DNR は「SW が生成した応答」「CacheStorage から返った応答」には効かない
  （SW 内の `fetch()` には効く）。R1 の受け入れ確認をする前に SW を消しておくほうが、観測が素直になる。
- **R2 との関係は無い。** MAIN world の `fetch` パッチはページ側にあり、SW より手前で応答を受け取る。
  SW が CacheStorage から返しても、パッチは通る。逆に content script は他オリジンの SW の中には入れない。

## 5. 実機で潰す手順

順序に意味がある。1 で仮説が外れたら 2 以降は無駄になる。

1. **そもそも生きているか。** `chrome://serviceworker-internals` で `www.youtube.com` の登録を開き、
   watch ページで 10 分再生する間 `RUNNING` のままかを見る。`STOPPED` に落ちるなら §2 の筋書きは外れで、
   別の増加源を探すべきである。ついでに `sw.js` を保存して fetch ハンドラの有無を読む。
2. **何が重いか。** その SW を inspect → Memory → ヒープスナップショットを t=0 / 30 / 60 分で 3 枚取り、
   Comparison ビューで「増えた分」のコンストラクタを見る。ここで初めて「不要なメモリ」の中身が分かる。
3. **どのプロセスの数字か。** Chrome のタスクマネージャで `Service worker: https://www.youtube.com/` の
   Memory footprint / JavaScript memory / **プロセス ID** を watch タブの行と比べる。
   同じ PID なら「SW だけ減らす」という話にはならない。
4. **消したときの差分。** 手で先に試す。DevTools → Application → Service Workers → Unregister、
   またはコンソールで `navigator.serviceWorker.getRegistrations().then(rs => rs.forEach(r => r.unregister()))`
   の後にリロード。これで §6 の 60 分プロトコルを **素 / 拡張 / 拡張 + SW 無し** の 3 構成で回し、
   傾きを比べる。**効果が出なければ実装しない。**
5. **壊れないかの確認。** 再生・シーク・ライブの継続再生・チャット受信・ログイン状態・視聴履歴の記録・
   広告からの本編復帰・SPA 遷移。R1 の受け入れ条件と同じ一覧に「通知が来ないこと」を足したもの。

## 6. 未確定事項（着手前に潰す）

1. YouTube の `sw.js` が `fetch` ハンドラを持つか（持たないなら §2 の筋書きが崩れる）
2. SW のプロセスが watch タブの renderer と同居しているか
3. `sw.js` のリクエストがどの `resourceTypes` で届くか
4. 登録を消したときに壊れるものがあるか（§5-5）
5. Web Push 通知を捨ててよいか（ユーザー判断）

上記 1〜4 は §5 の手順を 1 回通せば同時に潰せる。拡張のコードを書く前に済むものばかりである。

## 7. 出典

- [Using Service Workers — MDN](https://developer.mozilla.org/en-US/docs/Web/API/Service_Worker_API/Using_Service_Workers)
  （制御下ページのクロスオリジン取得も `fetch` イベントになること）
- [ServiceWorker memory footprint consistently increasing — Chromium issue 40260507](https://issues.chromium.org/issues/40260507)
- [chrome.browsingData — Chrome for Developers](https://developer.chrome.com/docs/extensions/reference/api/browsingData)
  （`origins` + `serviceWorkers`）
- [chrome.declarativeNetRequest — Chrome for Developers](https://developer.chrome.com/docs/extensions/reference/api/declarativeNetRequest)
  （SW が生成した応答・CacheStorage 由来の応答には効かない / SW 内の `fetch()` には効く）
- [The extension service worker lifecycle — Chrome for Developers](https://developer.chrome.com/docs/extensions/develop/concepts/service-workers/lifecycle)
  （idle 30 秒。ただし記述は拡張の SW について。Web の SW も同様の idle 打ち切りを持つ）
