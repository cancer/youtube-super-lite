# Issue #16 実装ガイド — ライブ SABR 詰みを公式 IFrame プレーヤー(WebView2)で救う

対象: [Issue #16](https://github.com/cancer/youtube-super-lite/issues/16) を実装する人。
このガイドは**迷ったら立ち返る場所**。方針・PoC 結果は確定済みなので、実装中に
「iframe か SABR 自前か」を再検討する必要はない（結論は §0）。判断の理由(Why)は各所に書く。

前提知識: Rust の所有権/借用、Win32 の親子ウィンドウ、このリポジトリのビルド手順(README)、
[child-dcomp-overlay-redesign.md](child-dcomp-overlay-redesign.md)（現行オーバーレイが子窓+DComp で組まれている経緯）。

---

## 0. これは何の工事か（PoC で確定した事実だけ）

ログイン済みでも**ライブ配信が再生できない**。YouTube が 2026-07-04 に TV client のライブ応答を
SABR 化し、`hlsManifestUrl` が消えて `serverAbrStreamingUrl` のみになったため。VOD は無影響（従来 mpv）。

PoC は2本とも完了し、方針は確定している:

- **採用＝公式 IFrame プレーヤーを WebView2 で埋め込む hybrid fallback。** VOD とライブ HLS が返る
  ものは従来どおり mpv、**ライブで SABR しか返らず詰むものだけ WebView2 の公式 embed** に流す。
  ログイン済み WebView2 セッションで、現状 YSL が再生不可の実ライブでも bot ゲート通過・再生を
  実証済み（[PoC #2](https://github.com/cancer/youtube-super-lite/issues/16#issuecomment-4917689955)）。
- **却下（保険扱い）＝案1(Rust ネイティブ SABR)。** `tools/p0-sabr-probe` で「PoToken 不要・nsig のみで
  UMP メディア受信」までは実証したが、再生パイプライン(P3〜P5, 3〜4週)が未実装。iframe が通る以上、
  緊急性はない。この工事では**触らない**。

この工事は「WebView2 を足す」作業ではなく、**再生バックエンドを2つ持ち、resolve の結果で
決定的に切り替える**作業。切替の判定材料はすべて既存 resolve が持っている（§2）。

### Super Lite 思想との整合（迷ったら）

「Super Lite」は公式 Web UI の富豪的リソース使用からの脱却（軽量化）が主旨で、広告回避は意図しない
（[[ysl-super-lite-intent]]）。embed は広告込みの正規利用で ToS クリーン、かつ watch ページ全体より軽い。
mpv より重いので**ライブ限定 fallback に留め**、VOD の最軽量 mpv 経路を常態に保つ。

---

## 1. 最終アーキテクチャ

```
winit 親窓 (wid) ── HWND: src/ui/shell.rs:140
  ├─ mpv 出力子窓      … Player::new_embedded(wid)   shell.rs:141（mpv が内部生成）
  ├─ DComp オーバーレイ子窓 … DcompOverlay::new(wid)  shell.rs:191 / dcomp_overlay.rs:585
  └─ WebView2 ホスト子窓   … 【本工事で新設】3枚目の WS_CHILD
```

再生バックエンドは排他:

| コンテンツ | バックエンド | 子窓の状態 |
|---|---|---|
| VOD / HLS が返るライブ | **mpv**（従来） | WebView2 hide・オーバーレイ表示 |
| SABR しか返らないライブ | **WebView2 公式 embed** | WebView2 表示・オーバーレイ hide |

切替の唯一の判定材料 = **resolve が `hlsManifestUrl` を返せたか**（[clients.rs:182](../crates/ysl-core/src/yt/resolve/clients.rs#L182) `hls_manifest()` が `Option`）。
ライブ(`is_live=true`)かつ HLS が取れない ＝ SABR 詰み ＝ WebView2 経路。それ以外は mpv。

---

## 2. 実装前に押さえる既存コードの地図（file:line）

再検討不要。ここに書いた場所だけ触る。

**ウィンドウ**
- 親窓 HWND: [shell.rs:140](../src/ui/shell.rs#L140) `let wid = hwnd_of(&window)?;`。`NativeRunning.parent_wid`([shell.rs:52](../src/ui/shell.rs#L52)) に保持。
- mpv 子窓: mpv が `wid` 内に自前生成（自前 CreateWindowEx なし）。[player.rs:20](../crates/ysl-core/src/player.rs#L20) `new_embedded`。
- オーバーレイ子窓生成: [dcomp_overlay.rs:585](../src/dcomp_overlay.rs#L585) `CreateWindowExW`、style `WS_CHILD|WS_VISIBLE|WS_CLIPSIBLINGS`([:589](../src/dcomp_overlay.rs#L589))。**WebView2 子窓は DcompOverlay::new と同じ要領で作る**。
- z-order 調整: [dcomp_overlay.rs:687](../src/dcomp_overlay.rs#L687) `ensure_topmost`（毎フレーム、上に別窓があれば `SetWindowPos(HWND_TOP)`）。
- リサイズ追従: [shell.rs:628](../src/ui/shell.rs#L628) `WindowEvent::Resized`（現状 overlay の resize のみ）。
- **`ShowWindow` / `SW_HIDE` / `SW_SHOW` はリポジトリ全体で未使用**。子窓の hide 手段は現状ゼロ。§PR4 で新設する。

**オーバーレイの可視モデル（重要）**
- 可視は「子窓 hide」ではなく [`render(active, ...)`](../src/dcomp_overlay.rs#L883) の `active=false` で**透明に描く**方式。子窓自体は常に `WS_VISIBLE` で全入力を所有（`HTTRANSPARENT` 貫通しない）。
- `active` は [shell.rs:440](../src/ui/shell.rs#L440) で「一覧/チャット/EQ 開いている or 3秒以内に操作」から算出。
- ∴ WebView2 モードで「オーバーレイを退ける」には `active=false` では**不足**（透明でも入力を吸う）。子窓を実際に hide する必要がある（§PR4）。

**resolve / 再生**
- 判定関数: [clients.rs:182](../crates/ysl-core/src/yt/resolve/clients.rs#L182) `hls_manifest(streaming) -> Option<String>`。
- ライブ判定: [clients.rs:171](../crates/ysl-core/src/yt/resolve/clients.rs#L171) `isLive`。resolve→playback へは [ResolveUpdate::Meta{is_live}](../crates/ysl-core/src/yt/resolve/mod.rs#L37) → [playback.rs:193](../crates/ysl-core/src/playback.rs#L193) `pb.is_live`。
- ログイン中ライブの解決: [mod.rs:307](../crates/ysl-core/src/yt/resolve/mod.rs#L307) `resolve_one` の TVHTML5+Bearer 腕。ここが「HLS が返れば Resolved、返らなければ…」の分岐点。
- mpv への URL 引き渡し: [playback.rs:183](../crates/ysl-core/src/playback.rs#L183) `loadfile`。
- `ResolveUpdate` enum: [mod.rs:37](../crates/ysl-core/src/yt/resolve/mod.rs#L37)（`Ready/Fallback/Meta/Error`）。**WebView2 経路は新しい variant か Meta 拡張で通知する**（§PR3）。

**入力合流点**
- [actions.rs:409](../src/ui/actions.rs#L409) `apply_action(&mut self, a: UiAction) -> bool` が3系統（オーバーレイ/dev-tools/キーボード）の唯一の合流点。モード切替の腕はここに足す。

**依存**
- WebView2 系の依存は**現状ゼロ**（[Cargo.toml:41](../Cargo.toml#L41) に `windows` crate はある）。追加する（§PR1）。

---

## 3. PR 分割（厳守）

> **1 PR = 1 スコープ。下の粒度を勝手に束ねない。** 複数スコープを1つの PR に詰めたら差し戻す。
> スタック PR で構わない（前の PR のマージを待たず次を積んでよい）。各 PR は単体で意味が完結し、
> レビュー可能な最小単位であること。

| PR | スコープ | 完了条件（これだけ） | issue #16 条件 |
|---|---|---|---|
| **PR1** | WebView2 子窓の生成 + embed HTML の正規 origin 配信（`--webview-probe` フラグで opt-in・§4.0） | フラグ付き起動で3枚目の子窓に自前 HTML(iframe embed) が**エラー153なしで**ロードされ描画される（匿名でよい＝bot ゲートは想定内）／無指定時は従来と同一挙動 | A + C |
| **PR2** | ログイン cookie の永続化（WebView2 内 Google ログイン） | ログイン済みプロファイルで PoC 対象ライブが bot ゲートなく再生 | B |
| **PR3** | 経路切替（mpv ⇄ WebView2）を resolve に配線 | SABR 詰みライブが自動で WebView2 に、VOD/HLS が mpv に流れる | D |
| **PR4** | UI 統合（オーバーレイ子窓 hide + mpv 隠し） | WebView2 モードでオーバーレイが退き入力が WebView に通る／mpv モードで従来どおり | E |
| **PR5** | fallback-of-fallback（`onError` → 「YouTubeで開く」） | 埋込無効(101/150)・年齢制限・メン限で既定ブラウザに最終 fallback | F |

依存順は PR1→PR2→PR3→PR4→PR5。PR1+PR2 で「手動で WebView に URL を入れれば再生できる」、
PR3+PR4 で「アプリが自動で切り替えて使える」、PR5 で「詰みケースも袋小路にしない」。

---

## 4. PR1 詳細 — WebView2 子窓 + embed 配信（今回の初手）

**ゴール**: winit 親窓 `wid` の3枚目の子窓に WebView2 をホストし、**自前 HTML に置いた
`<iframe src="…/embed/<id>">` を正規 origin/Referer で配信**して、エラー153を踏まずに
プレーヤーが描画されることを確認する。匿名で bot ゲートに当たるのは想定どおり（PR2 で解消）。
**このPRで再生成功まで求めない**（153回避と描画確認がゴール）。

### 4.0 CLI フラグで opt-in にする（独立マージ可能性の条件・必須）

**WebView2 子窓を全起動で無条件に生成してはならない。** 生成すると、通常の VOD 視聴時にも
全画面サイズの WebView2 子窓が mpv と競合し（z-order は [ensure_topmost](../src/dcomp_overlay.rs#L687)
がオーバーレイを毎フレーム最前面へ引くため不定）、かつ `autoplay` フラグにより隠れた WebView が
音を鳴らし得る。これでは PR1 を単体で main にマージできない。

→ **新規 CLI フラグ `--webview-probe`（[main.rs:52](../src/main.rs#L52) の引数パースに追加）で opt-in にする。**
フラグ無指定時は `WebviewHost::new` を呼ばず、従来と完全に同一挙動（既存の `--dcomp`/`--native` は
no-op 化済み [main.rs:64](../src/main.rs#L64)。同じ「実験機能はフラグ排他」の前例が
[child-dcomp-overlay-redesign.md](child-dcomp-overlay-redesign.md) 手順1）。
- フラグは `CliArgs` に足し、`NativeApp::init` に渡して `WebviewHost::new` 呼び出しを条件化する。
- **プローブ時は音を鳴らさない**（[[ysl-debug-mute]] と整合）: iframe の `autoplay=1` を付けない、
  または `mute=1` で起動する。`--autoplay-policy` フラグ自体は Environment 生成時に入れてよい
  （音を出す条件は URL 側の `autoplay`/`mute` で制御する）。

### 4.1 依存追加

`Cargo.toml` に `webview2-com`（`windows` crate と併用）。既存の `windows` crate と feature が
噛み合うバージョンに固定する（実装時に `cargo build` で確定。ここでバージョンをハードコードしない）。

### 4.2 Environment 生成（autoplay=条件C はここで畳む）

`--autoplay-policy` は **Environment 生成時にしか渡せない**（後付け不可）。YSL は Environment を
自前生成するので、PR1 の生成コードにこのフラグを入れておく（C を PR1 に同梱する理由）。

```rust
use webview2_com::{Microsoft::Web::WebView2::Win32::*, *};

let options = CoreWebView2EnvironmentOptions::default();
unsafe {
    // 条件C: 音声付き autoplay を許可（referrerpolicy だけでは不足）
    options.set_additional_browser_arguments("--autoplay-policy=no-user-gesture-required")?;
    // 条件B の下地: OS ログインは使わず、専用プロファイルに cookie を溜める（PR2 で本格運用）
    options.set_exclusive_user_data_folder_access(true)?;
}
// 第2引数 = UserDataFolder（%APPDATA%\YouTubeSuperLite\webview2 等の固定パス）
CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
    Box::new(move |handler| unsafe {
        CreateCoreWebView2EnvironmentWithOptions(None, Some(user_data_dir), &options, &handler)
            .map_err(webview2_com::Error::WindowsError)
    }),
    Box::new(move |code, env| { code?; tx.send(env).unwrap(); Ok(()) }),
)?;
```

> `set_additional_browser_arguments` は空白区切りで複数フラグを渡せる。ここに詰め込みすぎない
> （autoplay 以外を足すのは別 PR で理由付きで）。UserDataFolder のパスは固定（起動ごとに変えない＝
> cookie を使い回す前提。PR2 の核）。

### 4.3 子窓を作って Controller を貼る

`DcompOverlay::new`([dcomp_overlay.rs:585](../src/dcomp_overlay.rs#L585)) と同じ要領で `wid` を親に
`WS_CHILD` 子窓を作り、その HWND に `CreateCoreWebView2Controller(hwnd)` で WebView2 を貼る。
Controller の `Bounds` を子窓のクライアント矩形に合わせ、`Resized`([shell.rs:628](../src/ui/shell.rs#L628))
で追従させる（オーバーレイと同じ流儀）。

生成タイミングは `NativeApp::init`([shell.rs:132](../src/ui/shell.rs#L132)) の `DcompOverlay::new` の隣。
ハンドルは `NativeRunning`([shell.rs:48](../src/ui/shell.rs#L48)) に新フィールドで保持。

> **PR1 の時点では hide 手段は要らない**（描画確認が目的で、まだ mpv と併存させない）。
> hide/mode 切替は PR4。ここで ShowWindow を足すと PR4 のスコープを食う＝差し戻し対象。

### 4.4 自前 HTML を正規 origin で配信（条件A＝153回避の本丸）

エラー153(Video Player Configuration Error)は「embed をトップレベル文書として開く」と「Referer が
付かない」の2つで踏む。**両方を避ける**:

1. **トップレベル navigate しない**。自前 HTML に iframe を置く（構成B）:
   ```html
   <iframe src="https://www.youtube.com/embed/<VIDEO_ID>?enablejsapi=1"
           referrerpolicy="strict-origin-when-cross-origin"
           allow="autoplay; encrypted-media; fullscreen; picture-in-picture"
           style="border:0;width:100%;height:100%"></iframe>
   ```
   `allow` に `autoplay` 必須（条件C。§4.2 のフラグと両輪）。
2. **正規 origin/Referer を与える**。`NavigateToString`(about:blank=null origin) や data/blob URL は
   Referer が付かず153を踏むので**不可**。`SetVirtualHostNameToFolderMapping` で仮想ホスト
   （例 `https://ysl.embed.example/`。`.local` は mDNS 予約なので避ける）にローカルフォルダを
   マップし、その `https://<host>/player.html` を `Navigate` する。
   ```rust
   webview.SetVirtualHostNameToFolderMapping(
       w!("ysl.embed.example"), w!(<local_folder>),
       COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_ALLOW)?;
   webview.Navigate(w!("https://ysl.embed.example/player.html"))?;
   ```
   > 既定は `www.youtube.com/embed`。`youtube-nocookie.com/embed` は cookie(=ログイン文脈)を
   > 捨てるトレードオフなので診断用のみ（PR2 のログインと矛盾する）。

### 4.5 PR1 の検証

- 匿名で `player.html` をロードし、**エラー153が出ないこと**（＝origin/Referer が効いている）。
- プレーヤー UI が子窓に描画されること。bot ゲート（"Sign in to confirm you're not a bot"）が
  出るのは**想定どおり**（PR2 で解消）。ここで再生成功を条件にしない。
- 検証は dev-tools 経由でライブ URL を流す既存ルート（[[ysl-live-botgate]] の dev-tools メモ）を使う。
  現行ライブ ID は `curl -sL https://www.youtube.com/@NASA/live | grep -oE 'v=[A-Za-z0-9_-]{11}'`。

---

## 5. PR2〜PR5 の要点（詳細は各 PR 着手時にこの節を展開）

### PR2 — ログイン cookie の永続化（条件B）
- bot ゲート突破には **WebView2 プロファイルに YouTube ログイン cookie が必須**。匿名だと
  `LOGIN_REQUIRED` で詰む（PoC #1）。
- **OAuth token（YSL 本体が持つ）は Web cookie に変換できない**。→ WebView2 内で Google ログイン
  フローを通し、PR1 で固定した UserDataFolder に cookie を永続化して以後使い回す。
- 本物ブラウザ + ログイン cookie + BotGuard 自前実行で公式 web と同条件になり bot ゲート通過。

### PR3 — 経路切替（条件D）
- 判定は既存 resolve の `hlsManifestUrl` 有無で足りる（[clients.rs:182](../crates/ysl-core/src/yt/resolve/clients.rs#L182)）。
  `is_live && hls_manifest().is_none()` ＝ WebView2、それ以外 mpv。
- [resolve_one](../crates/ysl-core/src/yt/resolve/mod.rs#L307) の TVHTML5+Bearer 腕で HLS が取れなかった時、
  従来は Error に落ちる。ここで **WebView2 経路を指示する `ResolveUpdate`**（新 variant または
  `Meta` 拡張）を返し、[playback.rs](../crates/ysl-core/src/playback.rs) の `poll_resolve` が mpv `loadfile` の
  代わりに WebView2 に video_id を渡す分岐を持つ。
- `set_eq`([playback.rs:109](../crates/ysl-core/src/playback.rs#L109)) のコメントが予告する「mpv/webview 分岐」がここ。

### PR4 — UI 統合（条件E）
- **`ShowWindow` を新設**（現状ゼロ）。`DcompOverlay` に hide/show を足し、WebView2 モードでは
  オーバーレイ子窓を `SW_HIDE`、mpv モードで `SW_SHOW`。mpv 出力子窓も WebView2 モードで隠す。
- モード状態を `NativeRunning`([shell.rs:48](../src/ui/shell.rs#L48)) に持ち、[about_to_wait](../src/ui/shell.rs#L406) で
  子窓可視を切替。オーバーレイは1枚構成なので丸ごと hide/show で扱える（[[pr2-dcomp-overlay]]）。
- **EQ(#30/#31) は mpv 経路限定**。WebView では mpv `af` が効かず、オーバーレイ hide で EQ パネルも
  出ない。これは割り切り（クロスオリジンで iframe 内の音に外から触れない）。
- チャットは公式 embed（`live_chat`）に委ねる（独自オーバーレイを重ねると入力の取り合いが再燃）。

### PR5 — fallback-of-fallback（条件F）
- iframe に `enablejsapi=1`（PR1 で既に付与）＋ IFrame Player API の `onError` でエラーコード捕捉。
- 埋込無効(err 101/150)・年齢制限(embed はサインイン無関係に不可)・メン限 →
  **「YouTube で開く」（既定ブラウザ）** に最終 fallback。動機が「ログイン済みライブ」ゆえ
  この失敗ケースと重なるので必須。

---

## 6. やらないこと（スコープ外）

- **案1(Rust ネイティブ SABR)の再生パイプライン**。`tools/p0-sabr-probe` はそのまま。保険。
- **VOD 経路の変更**。mpv のまま無改修。
- **WebView 上に YSL 独自 UI を重ねる**（チャット等）。公式 embed に委ねる。
- **EQ を WebView に効かせる**。mpv 経路限定で割り切り済み。

関連: [[ysl-live-botgate]] [[ysl-super-lite-intent]] [[pr2-dcomp-overlay]] [[guide-pr-split-enforcement]]
