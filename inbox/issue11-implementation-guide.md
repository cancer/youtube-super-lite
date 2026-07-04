# Issue #11 実装ガイド — Controller/native_app の解体

対象: [Issue #11](https://github.com/cancer/youtube-super-lite/issues/11) を実装する人。
このガイドは**迷ったら立ち返る場所**として書かれている。設計判断はすべて確定済みなので、
実装中に「どっちがいいか」を再検討する必要はない。判断の理由(Why)は各所に書いてある。

前提知識: Rust の所有権/借用の基礎、`std::sync::mpsc`、このリポジトリのビルド手順(README)。

---

## 0. これは何の工事か

`Controller`(pub 62 フィールド)と `native_app.rs` は、**縦軸(ドメイン)と横軸(レイヤー)が
1つの入れ物に溶けた** God Class / God File。同じ struct・同じファイルの中では何でも触れるため、
以下の境界違反が実際に起きた(詳細は Issue #11):

1. UI 状態が Controller に混入 → 誰も読まないデッドフィールド 11 個
2. アプリ判断が native_app に混入 → 同一ロジックが3系統にコピペされ、既に挙動ドリフト
3. Controller が winit の型を保持 → 「UI 非依存コア」が実は不成立

この工事は 62 フィールドを小分けにする作業では**ない**。2つの軸を復元する作業:

- **縦**: account / playback / content / chat の4ドメイン。互いを知らない
- **横**: ドメイン(lib) ← flows(lib) ← UI(bin)。下は上を知らない。lib は winit に依存できない

## 1. 最終アーキテクチャ

```mermaid
graph TD
    subgraph bin["ysl (bin) — UI"
        shell["ui/shell.rs<br/>winit配線 + Win32の知恵(地雷)"]
        actions["ui/actions.rs<br/>apply_action(全入力の合流点)"]
        present["ui/present.rs<br/>状態→Card/ChatLine(純関数)"]
        overlay["dcomp_overlay<br/>描画+ヒットテストのデバイス"]
        devtools["devtools"]
    end
    subgraph lib["crates/ysl-core (lib) — winit 依存不可"]
        flows["flows.rs<br/>跨ぎ system 3本"]
        account["account.rs"]
        playback["playback.rs"]
        content["content.rs"]
        chat["chat.rs"]
        yt["yt/ (InnerTube API 群)<br/>auth/recommend/subscriptions/<br/>history/playlist/chat/resolve/..."]
        player["player.rs (libmpv)"]
    end
    shell --> actions --> flows
    present --> content
    flows --> account & playback & content & chat
    account & content & chat --> yt
    playback --> yt & player
```

### 語彙(この工事の全判断はこの3語で決まる)

| 語 | 意味 | 例 |
|---|---|---|
| **データ構造体** | 状態だけを持つ純構造体。**ロジックのメソッドを持たない**。struct 化してよいのは「1つの状態機械」だけ(基準は design-principles.md 原則1) | `Feed<T>`, `Account`, `Playback` |
| **system** | 状態を処理する関数。触る状態を**引数で宣言**する | `content::poll(&mut Content) -> bool` |
| **shell** | winit/OS との境界。状態を**所有**するがロジックは持たない | `ui/shell.rs` |

system は触るドメインの数で置き場が決まる(種類の違いではなく、依存の向きの帰結):
- **単一ドメインの system** → そのドメインのモジュール内(`content::poll` など)
- **複数ドメインを跨ぐ system** → ドメインモジュール内には置けない(ドメイン同士の import 禁止に
  抵触するため)。全ドメインが見える上位モジュール **`flows.rs`** に置く。
  現状ここに住む system は**3本だけ**であり、この本数は「アプリ全体の結合量の記録」でもある

> **Why この本数制限が生命線か**: flows は全ドメインに触れる唯一の場所。本設計は
> 「全状態を格納し任意の処理からアクセスできる共有ストレージ」(ECS 実装で言う `World`/`registry`、
> DI コンテナ、グローバル状態)を意図的に作らない — 状態は shell が普通のフィールドとして所有し、
> アクセスは各関数のシグネチャで列挙して渡す「配給制」。
> flows はその配給が集中する場所なので、ここにロジックが無制限に積み上がると
> 「万能アクセスの窓口 + ロジックの集積」= **旧 Controller(God Class)の再誕**になる。
> flows の本数制限はその再結晶を防ぐ堤防。

### 設計原則

本工事が従う原則(データと振る舞いの分離・情報隠蔽・明示的な依存・ID 参照)は、工事後も
全実装に適用される恒久ルールとして **[docs/design/design-principles.md](../docs/design/design-principles.md)**
に定めてある。**着工前に必ず読むこと**。本ガイドの絶対ルールはその原則の工事向け具体化。
なお「ECS を導入する工事」ではないので、bevy_ecs 等を探しに行かないこと(出自は原則ドキュメント参照)。

### 絶対ルール(違反したら PR は差し戻し)

1. **データ構造体にロジックのメソッドを生やさない**。読み取り用 getter(`items()`, `is_busy()`)だけは可
   (Rust にはフィールドの読み取り専用公開がないため。getter は振る舞いではなくアクセス制御)
2. 書き込みは**同モジュールの system 関数のみ**。フィールドは private にしてこれをコンパイラに守らせる
3. **flows.rs(跨ぎ system の置き場)に4本目を足さない**。足したくなったら、それは本当に跨ぎか
   (片方のドメイン内で閉じないか)を疑う。本当に跨ぎなら Issue を立てて相談 —
   跨ぎ system の増加はアプリの結合が増えたということなので、無言で足してはいけない
4. **ドメイン同士は import しない**。跨ぐデータ(トークン、video_id、再生位置)は flows が値で運ぶ
5. **境界を渡す参照は ID**(`video_id`/`channel_id`/`playlist_id`)。index(位置)を渡さない
6. **native_app の地雷コード(§2.3)は書き換えない・コメントを消さない・移動時は中身無変更**
7. `state_json` の **JSON キー名は変更禁止**(外部 dev-tools クライアントが消費。右辺の式だけ変える)
8. 各 PR は `cargo check` **警告 0**・`.\build.ps1` 成功・§2.2 のスモークを通してからレビューに出す

## 2. 共通の作業規律

### 2.1 スコープ規律

- 各 PR は本ガイドの担当範囲**だけ**をやる。「ついでに直したい」ものを見つけたら Issue 化して先に進む
- 挙動を変える箇所は本ガイドで明示指定されたもの(§8.3 のドリフト統一)だけ。それ以外は**挙動不変**が原則

### 2.2 検証(全 PR 共通)

```powershell
cargo check          # エラー0・警告0
.\build.ps1          # ビルド
# 実行して dev-tools でスモーク:
.\target\debug\youtube-super-lite.exe --enable-dev-tools
```

dev-tools は `http://127.0.0.1:<表示されたポート>` に立つ。使うのは:
- `GET /state` — UI 状態の JSON。**毎 PR で変更前後の JSON キー集合が同一なことを確認**
- `GET /action/<name>` — 全 UI 操作の注入(`open_subs`, `list_select`, `toggle_chat`, `login` 等。一覧は native_app の devtools_action)
- `GET /screenshot` — 見た目の確認

PR ごとの追加スモークは各 PR の節に「Done の定義」として書いてある。

### 2.3 native_app の地雷マップ(触るな・消すな)

以下は Win32/winit の暗黙知で、**コメントがコードより価値がある**。理由コメントごと保全する:

| 場所(現行行) | 内容 |
|---|---|
| `window_event` の `Resized`(1148-1161) | **ここで last_activity を更新してはいけない**。hwdec 切替が Resized を連発し、操作扱いすると自動非表示が壊れる(過去にデグレ実績) |
| `window_event` の `Focused`(1138-1147) | フォーカス喪失でオーバーレイを隠さない(非アクティブでもチャット表示継続) |
| `window_event` の `CursorMoved`(1127-1137) | 遮蔽時は届かない前提の設計。グローバル座標推測をしない |
| `about_to_wait` の可視判定(~970-975) | 3秒無操作の自動非表示。list_open/chat_open は例外 |
| スクショ処理(`pending_shot`/`shot_delay`) | 前面化→数フレーム待ち→キャプチャ。即時キャプチャは合成前の絵になる |
| `init` の HWND 取り出し〜mpv 埋め込み(117-137) | 順序依存(HWND→Player::new_embedded→Controller) |

### 2.4 このガイドの行番号について

行番号は**着工前の main 時点**のもの。PR S でファイルが移動しても中身は同一なので行番号は有効。
D1 以降は controller.rs が縮むため、**関数名を一次キー、行番号は参考**として扱うこと。

---

## 3. PR S — workspace 化 + 目覚まし抽象

**目的**: レイヤー境界(lib は winit を知らない)をコンパイラに守らせる箱を作る。**ロジック変更ゼロ**。

### 3.1 ディレクトリ構成

```
Cargo.toml                  # [workspace] + 既存 bin パッケージ
src/                        # bin 側(残留): main.rs, native_app.rs, controller.rs(暫定),
                            #   dcomp_overlay.rs, devtools.rs, settings.rs, design.rs, bin/
crates/ysl-core/
  Cargo.toml
  src/
    lib.rs                  # pub mod yt; pub mod player; ... + Waker 定義
    yt/                     # InnerTube API 群(旧 src/ 直下から移動)
      mod.rs, auth.rs, chat.rs, history.rs, playlist.rs,
      recommend.rs, subscriptions.rs, mark_watched.rs
      resolve/              # 旧 src/resolve/ ごと
    player.rs
    gpu_usage.rs
    image_cache.rs
    types.rs                # Quality / Codec(main.rs から移動。resolve が使うため lib 必須)
```

**bin に残すもの(理由つき)**:
- `UserEvent` — winit のイベント型。lib に置いたら本工事の意味がない
- `AuthMsg` / `CHAT_MAX_MESSAGES` — 使用者が controller.rs(bin 残留)のみ。D3/D2 で lib へ移す
- `settings.rs` — 中身が UI 設定(チャット幅・フォントサイズ)なので UI の持ち物
- `devtools.rs` / `dcomp_overlay.rs` / `design.rs` — UI そのもの

### 3.2 目覚まし抽象(Waker)

背景スレッドの「完了したからイベントループ起きて」を winit 非依存にする:

```rust
// crates/ysl-core/src/lib.rs
/// 背景スレッドがメインループを起こすためのコールバック。
/// bin 側で EventLoopProxy を包んで注入する。lib は winit を知らない。
pub type Waker = std::sync::Arc<dyn Fn() + Send + Sync>;
```

```rust
// bin 側(native_app の init)での生成:
let proxy = self.proxy.clone();
let waker: ysl_core::Waker =
    std::sync::Arc::new(move || { let _ = proxy.send_event(UserEvent::Background); });
```

**この PR で Waker 化するのは lib へ移るコードだけ**: `resolve::ResolverHandle::spawn(tx, proxy)`
→ `spawn(tx, waker)`(内部の `proxy.send_event(...)` を `waker()` に置換)。
controller.rs は bin に残るので proxy のままでよい(D1〜D4 で消える)。

### 3.3 手順

1. ルート Cargo.toml に `[workspace] members = ["crates/ysl-core"]` を追加(既存 `[package]` は残す)
2. `crates/ysl-core/Cargo.toml` を作成。依存は**移動したモジュールが実際に使うものだけ**を
   ルートの Cargo.toml から仕分けして書く(reqwest/serde/anyhow/libmpv 系など)。
   winit を書いたら設計違反 — ここがこの PR の存在意義
3. `git mv` でファイル移動(履歴保全のため必ず git mv)。yt/ 配下へ入れるので `mod` 宣言を整える
4. bin 側の `use crate::recommend::...` → `use ysl_core::yt::recommend::...` 等を全置換。
   `Quality`/`Codec` は `ysl_core::types::{Quality, Codec}` から re-export して main.rs の
   既存利用を壊さないようにしてもよい(`pub use` 1行)
5. Waker 定義 + resolve の置換
6. **コンパイラに聞く**: `cargo check` のエラーを潰し切る。「lib 側から bin の型が見えない」と
   言われたら、その型の置き場が間違っている(→3.1 の表に従う)

### 3.4 Done の定義

- [ ] `cargo check` 警告0 / `.\build.ps1` 成功
- [ ] `crates/ysl-core/Cargo.toml` に winit が**ない**
- [ ] 挙動不変: 動画再生・タブ1〜4・チャット・ログインが従来通り(dev-tools で確認)
- [ ] `src/bin/*` のプローブがビルドできる(独立バイナリなので影響ないはずだが確認)

---

## 4. PR D1 — Content(フィード4本 + playlist + avatars)

**目的**: 一番型が効く部分から、データ/system スタイルの書き味を確定させる。
以後の D2〜D4 はこの PR の作法を真似るだけになるので、**ここが本工事の型決め**。

### 4.1 enum 統一: `FeedUpdate<T>`

`RecommendUpdate` / `SubUpdate` / `HistoryUpdate` は同型(`SubUpdate` だけ variant 名が `Feed`)。
3つを削除して1つに:

```rust
// crates/ysl-core/src/content.rs
/// 一覧系 fetch の背景スレッド→メインの通知。旧 RecommendUpdate/SubUpdate/HistoryUpdate を統一。
pub enum FeedUpdate<T> {
    Items(Vec<T>),
    Error(String),
}
```

producer 側の変更(3ファイル、機械的):
- `yt/recommend.rs`: `RecommendUpdate` 削除。`fetch_home_feed(token, tx: &Sender<FeedUpdate<VideoItem>>)`
- `yt/subscriptions.rs`: `SubUpdate` 削除。`SubUpdate::Feed(items)` を送っていた箇所は `FeedUpdate::Items(items)`
- `yt/history.rs`: `HistoryUpdate` 削除
- `PlaylistUpdate`(二階層)と `ChatUpdate`(NotLive あり)と `ResolveUpdate`(多段)は**対象外。触らない**

### 4.2 `Feed<T>` — 純データ + system 関数

```rust
/// 非同期取得する一覧の共通状態。フィールドは private(書き込みは本モジュールの関数のみ)。
pub struct Feed<T> {
    items: Vec<T>,
    tx: Sender<FeedUpdate<T>>,
    rx: Receiver<FeedUpdate<T>>,
    busy: bool,
    label: &'static str,   // エラーログの識別用(例: "recommend")
}

impl<T> Feed<T> {
    pub fn new(label: &'static str) -> Self { /* channel() して組む */ }
    // 読み取り getter(bin の present が使う)
    pub fn items(&self) -> &[T] { &self.items }
    pub fn is_busy(&self) -> bool { self.busy }
}

/// system: rx を drain して取り込む。新しい Items が来たら true。
/// Error は eprintln!("[{label}] 取得エラー: {e}") で記録するだけ(status フィールドは持たない — 旧実装の
/// status は全機能で誰にも読まれていないデッドだったため、再現しない)。
pub fn poll_feed<T>(f: &mut Feed<T>) -> bool { /* try_recv ループ */ }

/// system: 取得開始の帳簿(busy=true, items.clear())をして、spawn 用に tx の clone を返す。
pub fn begin_fetch<T>(f: &mut Feed<T>) -> Sender<FeedUpdate<T>> { /* ... */ }
```

**Why `poll_feed -> bool`**: callback を受ける形(`poll(|items| ...)`)にすると closure が `&mut self`
を要求して借用が衝突する。bool を返し、呼び出し側が「更新されたら items() を読んでアバター依頼」と
続けるのが借用安全(検証済み)。

### 4.3 content モジュールのデータ構造体(袋 struct は作らない)

**`Content` という束ね型は作らない**。content ドメインの中身は互いに不変条件を共有しない
独立した状態機械の集まりなので、束ねると system のシグネチャが袋全体を要求してしまい
「依存の明示」の解像度が落ちる(design-principles.md 原則1の struct 化基準)。
content.rs モジュールには以下の**型たち**を定義し、所有は呼び出し側(過渡期 Controller → D4 で shell)が
個別フィールドで持つ:

```rust
// それぞれが1つの状態機械。フィールドは全部 private
pub struct Feed<T> { ... }                        // §4.2。インスタンスは recommend/subs/history の3つ
pub struct ChannelView { feed: Feed<VideoItem>, title: String }   // open_channel で同時に変わる対
pub struct Playlist {                              // 二階層ナビゲーションという1つの機械
    lists: Vec<PlaylistSummary>,
    items: Vec<PlaylistItem>, items_title: String,
    tx: Sender<PlaylistUpdate>, rx: Receiver<PlaylistUpdate>,
    busy: bool,
}
pub struct AvatarCache {                           // 解決済み+依頼済みの整合が不変条件
    map: HashMap<String, String>,
    requested: HashSet<String>,
    tx: Sender<(String, String)>, rx: Receiver<(String, String)>,
}
```

読み取り getter: `Feed::items()`/`is_busy()`、`ChannelView::items()`/`title()`、
`Playlist::lists()`/`items()`/`items_title()`/`is_busy()`/`is_items_view()`
(= `!items.is_empty()`。native_app の階層判定を置き換える)、`AvatarCache::url_for(&str)`。

**削除されるもの(移植しない)**: `recommend_status` `sub_visible` `sub_status` `history_visible`
`history_status` `playlist_visible` `playlist_status` `channel_busy`(全てデッド。grep 確認済み)。

### 4.4 system 関数(移植元マッピング)

シグネチャは**触る機械だけ**を列挙する(袋を渡さない):

| 新関数(content.rs) | 移植元(controller.rs) | 備考 |
|---|---|---|
| `poll_feed(&mut Feed<T>, &mut AvatarCache) -> bool` | poll_recommend(461)/poll_channel(478)/poll_subs(604)/poll_history(666) | 「Items 到着→チャンネル名収集→アバター依頼」の連鎖は content モジュール内で完結させる(旧実装は Controller が配線していたが、これはドメインの内部事情)。avatars を触るのでシグネチャに現れる |
| `poll_avatars(&mut AvatarCache)` | poll_channel_avatars(553) | |
| `request_avatars(&mut AvatarCache, names, &Waker)` | request_channel_avatars(561) | 二重依頼防止の HashSet ロジックごと移植 |
| `start_recommend(&mut Feed<VideoItem>, token: &str, &Waker)` | start_recommend(587) | **token は &str 引数で受ける**(account を import しない — 絶対ルール4) |
| `start_subs(&mut Feed<SubVideo>, ...)` / `start_history(&mut Feed<HistoryItem>, ...)` | start_subs(624)/start_history(684) | busy ガード(`if busy { return }`)は現挙動通り subs/history/playlist のみ。**recommend に busy ガードを足さない**(挙動維持) |
| `open_channel(&mut ChannelView, name, &Waker)` / `open_channel_by_id(&mut ChannelView, id, title, &Waker)` | 493/515 | spawn クロージャの中身はそのまま流用 |
| `start_playlist_list(&mut Playlist, token, &Waker)` / `start_playlist_items(&mut Playlist, id, title, token, &Waker)` | 730/755 | |
| `poll_playlist(&mut Playlist)` | poll_playlist(706) | |
| `back_to_lists(&mut Playlist)` | (native_app 625-626, 1330-1331 の直接 `.clear()` を関数化) | items と items_title を同時にクリア |

spawn の完了通知は `proxy.send_event` → `waker()` に置換(全 start_* 共通)。

### 4.5 過渡期の配線(この PR の間だけ)

Controller は残っている。`Controller` に `pub recommend: Feed<VideoItem>`, `pub channel_view: ChannelView`,
`pub subs: Feed<SubVideo>`, `pub history: Feed<HistoryItem>`, `pub playlist: Playlist`,
`pub avatars: AvatarCache` を持たせ、旧フィールド群を削除。
Controller 内の呼び出し(`poll_auth` がログイン時に呼ぶ `start_recommend` 等)は
`content::start_recommend(&mut self.recommend, &token, &self.waker)` 形式に書き換える
(Controller にも waker を1本持たせる。D4 で消えるので雑でよい)。

### 4.6 native_app の変更サイト

`self.core.recommend_items` → `self.core.recommend.items()` 等。対象:
- `list_rows()`(228-349): 5ソース分の items 参照 + `channel_title` + playlist 2階層分岐
- `ensure_source_fetched()`(351-379): 空チェック + busy ガード。
  recommend のガードは現状維持: `items が空 && tokens.is_some()`
- `avatar_for()`(225)
- playlist の直接 `.clear()` 2箇所(625-626, 1330-1331)→ `back_to_lists()` 呼び出しへ
- `play_list_index()`(738-760)と Enter ハンドラ(1295-1304)の `playlist_items.is_empty()` → `is_items_view()`

### 4.7 Done の定義

- [ ] 共通検証(§2.2)+ `RecommendUpdate`/`SubUpdate`/`HistoryUpdate` が grep で 0 件
- [ ] dev-tools: `/action/open_recommend|open_subs|open_history|open_playlist` で各タブ表示
- [ ] playlist: 一覧→Enter で中身→`/action/list_back` で一覧に戻る(2階層ナビ)
- [ ] カードの丸アイコン(アバター)が出る
- [ ] チャンネル名クリック相当(`OpenChannelOf`)でチャンネルビュー→戻る

---

## 5. PR D2 — Chat

**目的**: 最小ドメインで D1 の作法を反復。

### 5.1 データと system

**1 動画 : 1 インスタンス**(design-principles.md「寿命は現実に合わせる」)。
アプリ寿命の Chat 構造体は作らず、接続ごとに `ChatSession` を生成して捨てる:

```rust
// crates/ysl-core/src/chat.rs(ドメイン)。yt/chat.rs(ポーラー)とは別物なので注意
pub struct ChatSession {
    messages: Vec<yt::chat::ChatMessage>,
    rx: Receiver<yt::chat::ChatUpdate>,   // チャネルはセッション生成時に作る(共有しない)
    stop: yt::chat::ChatStop,
    status: String,      // 唯一生きている status(native_app が「チャットが有効か」の判定に読む)
}
pub fn start(video_id: String, offset: Arc<AtomicI64>, waker: &Waker) -> ChatSession  // 旧 start_chat(442)
pub fn poll(s: &mut ChatSession) -> bool  // 旧 poll_chat(418)。NotLive を受けたら false を返す
impl Drop for ChatSession { /* stop.stop() — RAII。旧 stop_chat(775) は関数ごと消える */ }
```

- 保持側(過渡期 Controller → shell)は `chat: Option<ChatSession>`。
  停止 = `self.chat = None`(Drop がポーラーを止める)。NotLive で poll が false → 同じく None 代入
- チャネルをセッション内で生成するため、**前の動画のポーラーが遅れて送るメッセージは
  破棄済み rx と一緒に構造的に死ぬ**(messages.clear() の儀式も、混入バグの余地も消える)

- `CHAT_MAX_MESSAGES` を main.rs からこのモジュールへ移す(使用者がここだけになる)
- `chat_visible` は移植しない(デッド。native_app は自前の `chat_open` で管理している)
- getter: `messages()`, `available() -> bool`(= `!status.is_empty()`。
  native_app 709/989 の `!chat_status.is_empty()` をこれに置換)
- `offset`(再生位置の Arc)は**引数で注入**。Chat が Playback を知る理由にしない(絶対ルール4)

### 5.2 Done の定義

- [ ] 共通検証 + ライブ動画でチャット表示・スクロール(`/action/chat_scroll_up` 等)
- [ ] VOD(非ライブ)で NotLive → チャット欄が静かに閉じる(従来挙動)

---

## 6. PR D3 — Account(+ イベント返し方式の導入)

**目的**: auth の移設と、**跨ぎフローを「イベントを返して呼び出し側が routing する」形に外出し**する。
これは D4 で作る flows の先行形であり、この PR の設計上の主眼。

### 6.1 データと system

寿命で2つに割る(design-principles.md「寿命は現実に合わせる」):
**credentials はアプリ寿命**(「今誰としてログインしているか」はアプリ全体につき1つの事実)、
**進行中の操作は per-operation**(`Option<AuthTask>`。チャネルはタスクごとに生成):

```rust
// crates/ysl-core/src/account.rs
pub struct Account {
    tokens: Option<yt::auth::Tokens>,   // credentials(アプリ寿命)
    channel: Option<String>,
    status: String,      // native_app が表示に読む(719, 979)ので生きている
    backend: String,     // login/refresh の API エンドポイント
    task: Option<AuthTask>,   // 進行中の login/like。busy フィールドは is_busy()=task.is_some() に置換
}
struct AuthTask { rx: Receiver<AuthMsg> }   // tx はタスク開始時に生成して spawn に渡す
pub enum AccountEvent { LoggedIn }   // 呼び出し側が反応すべき出来事だけ。Like 完了等は status 更新で完結
pub fn poll(a: &mut Account) -> Vec<AccountEvent>    // 旧 poll_auth(334)から「跨ぎ部分を抜いた」もの。完了で task=None
pub fn start_login(a: &mut Account, waker: &Waker)           // 旧 378
pub fn start_silent_login(a: &mut Account, rt: String, waker: &Waker)  // 旧 398
pub fn start_like(a: &mut Account, video_id: String, waker: &Waker)    // 旧 782
```

`AuthMsg` を main.rs からこのモジュールへ移す。
getter: `token() -> Option<&str>`, `is_busy()`(= task.is_some()), `status()`, `channel_name()`, `is_logging_in()`。
チャネルがタスクごとなので、古い試行の遅延応答が新しい試行に混入する余地が構造的にない。

### 6.2 跨ぎフローの外出し(重要)

旧 `poll_auth` は LoggedIn を受けたとき **auth の外の仕事**を3つやっていた(controller.rs:348-355):
履歴の再送・おすすめ先読み・保留 URL の解決開始。これらは Account の知識ではないので、
新 `poll` は **`AccountEvent::LoggedIn` を返すだけ**にする(トークン保存・status 更新など
auth 内で閉じる処理は poll 内でやる)。

受け手(過渡期は Controller、D4 以降は shell)がイベントを routing する:

```rust
for ev in account::poll(&mut self.account) {
    match ev {
        AccountEvent::LoggedIn => {
            // 旧 348-355 の3つの仕事をここで(D4 で flows::on_logged_in に昇格する)
        }
    }
}
```

fire-and-forget 系(`save_watch_later`(533)/`send_card_feedback`(543)/`start_mark_watched_if_logged_in`(232))
は「token を貰って spawn するだけ」の関数として account.rs に移す(mark_watched は
`mark_watched(token: &str, url: &str)` 形式にして current_url への依存を引数化)。

### 6.3 Done の定義

- [ ] 共通検証 + `/action/login` でブラウザ承認→「ログイン中: <名前>」表示
- [ ] 再起動でサイレントログイン復元→おすすめが自動で先読みされる
- [ ] CLI 引数で URL 指定起動→ログイン完了後に再生が始まる(保留→解決の連鎖。この時点では
      Controller 内の routing で動いていること)
- [ ] `/action/like` で高評価(status に反映)

---

## 7. PR D4 — Playback + flows + Controller 消滅

**目的**: 最後のドメインを移し、跨ぎ方針を flows として確定し、Controller を削除する。

### 7.1 Playback データと system

```rust
寿命で2層に割る(design-principles.md「寿命は現実に合わせる」)。
**装置と好みはアプリ寿命**(mpv は窓に埋め込まれたデバイス、ResolverHandle は base.js/boa
キャッシュを持つ常駐ワーカーで、どちらも作り直せない/作り直したら設計意図が消える。
quality/codec はユーザー設定)。**再生セッションは 1 URL : 1 インスタンス**:

```rust
// crates/ysl-core/src/playback.rs
pub struct Playback {
    // ── 装置と好み(アプリ寿命)──
    player: player::Player,
    resolve_handle: resolve::ResolverHandle,
    quality: Quality, codec: Codec,
    player_offset_ms: Arc<AtomicI64>,
    gpu_monitor: Option<gpu_usage::Monitor>,
    // ── 再生ごとに丸ごと差し替え ──
    session: Option<PlaySession>,
    pending_resolve: Option<String>,   // auth レースで「まだセッションになれない URL」(Why は旧 96-99 のコメントを保全)
}
struct PlaySession {
    url: String,
    is_live: bool,
    reply_rx: Receiver<resolve::ResolveUpdate>,   // ★セッションごとに生成し、request に tx を同梱する
    pending_fallback: Option<resolve::Resolved>,
    native_load_at: Option<Instant>,
    fallback_armed: bool,
}
pub fn poll_resolve(pb: &mut Playback)        // 旧 264。Ready→loadfile、Meta→is_live/title、Fallback 控え
pub fn check_fallback(pb: &mut Playback)      // 旧 297。3秒監視→サイドカー切替(状態機械ごと移植)
pub fn poll_gpu(pb: &mut Playback)            // 旧 647
pub fn start_resolve(pb: &mut Playback, url: String, token: Option<&str>)  // 旧 253 + セッション生成
pub fn hold(pb: &mut Playback, url: String)   // pending_resolve への保留
pub fn take_pending(pb: &mut Playback) -> Option<String>
```

- `start_resolve` は `self.session = Some(PlaySession::new(url, reply_rx))` で**丸ごと差し替える**。
  旧 load() の手動リセット儀式(controller.rs:200-207 の5フィールド初期化)は構造ごと消滅
- `resolve::ResolveRequest` に `reply: Sender<ResolveUpdate>` を追加し、常駐ワーカーは
  依頼に同梱された reply へ送る(ワーカー自体は常駐のまま)。**前のセッション宛の遅延応答は
  破棄済み rx と一緒に死ぬ** — 現行の共有チャネルに潜在する「動画切替後に古い Ready が
  届いて前の動画を再生する」レースがここで構造的に閉じる
- `load_error` と `resolve_busy` は移植しない(デッド)。
player への直接操作(pause/seek/volume 等、native_app から約30箇所)は
`pub fn player(&self) -> &player::Player` を生やしてそのまま通す(Player 自体は閉じた API なので
ラップし直さない — Issue 旧版から一貫してスコープ外)。quality/codec は UI が巡回変更するので
`set_quality`/`set_codec` の setter を用意(再解決の判断は flows/apply_action 側。PR B で一本化)。

### 7.2 flows.rs — 跨ぎ system の置き場(3本で全部。4本目は禁止)

```rust
// crates/ysl-core/src/flows.rs — 複数ドメインを触る system はこのファイルにしか置けない
// (ドメイン同士は import 禁止のため、跨ぐ処理は構造上ここに集まる)
/// ①ログイン確定: 履歴再送 + おすすめ先読み + 保留していた再生の解決(旧 poll_auth 348-355)
pub fn on_logged_in(acc: &Account, pb: &mut Playback, recommend: &mut Feed<VideoItem>, waker: &Waker)

/// ②再生開始: ログイン処理中なら保留(bot ゲート回避。旧 load 195-228 の判断部分)
pub fn play(pb: &mut Playback, acc: &Account, url: &str)

/// ③再生とチャットの連動: play + video_id 抽出 + chat 接続(native_app に4箇所コピペされていたコンボ)
pub fn play_with_chat(pb: &mut Playback, chat: &mut Option<ChatSession>, acc: &Account, url: &str, waker: &Waker)
```

### 7.3 Controller の削除と shell への所有移転

- `NativeRunning` が直接持つ: `account: Account, playback: Playback, chat: Option<ChatSession>, waker: Waker` +
  content の各機械(`recommend`/`channel_view`/`subs`/`history`/`playlist`/`avatars`)を個別フィールドで
  (袋 struct を作らない — design-principles.md 原則1)
- `poll_all()`(382-399)は system 呼び出しの列に書き換え:
  offset store → `account::poll`(イベント routing は `flows::on_logged_in` へ)→ `chat::poll` →
  content 各 poll → `playback::poll_resolve` → `check_fallback` → `poll_gpu`
- `init`(117-)の構築順は維持: HWND → Player → 各ドメイン構築 → gpu 監視 → サイレントログイン → 初期 URL
- `self.core.x` → `self.x` は一括置換でよい
- **controller.rs を削除**。`git rm` して mod 宣言も消す

### 7.4 Done の定義

- [ ] 共通検証 + controller.rs が存在しない
- [ ] フルスモーク: 再生開始 / ログイン待ち保留→自動再生(CLI 起動) / 画質切替→再解決 /
      native 失敗→サイドカー切替(壊れた URL で確認) / タブ1〜4 / チャット / ログイン
- [ ] `docs/design/architecture-overview.md` の「状態とロジックはすべて Controller に集約され」を
      実構造(ドメイン+flows+shell)に更新。`docs/design/threading-and-io.md` の enum 列挙も
      `FeedUpdate<T>` に修正

---

## 8. PR B — 入力3系統の一本化 + アクション ID 化

**目的**: 同一アクションが devtools_action / キーボード / OverlayAction match に
コピペされている状態を、単一の `apply_action` に合流させる。挙動ドリフトもここで解消する。

### 8.1 UiAction と合流点

```rust
// native_app.rs(PR U で ui/actions.rs へ移る)
enum UiAction {
    TogglePause, SeekTo(f64), SeekBy(f64), VolumeBy(f64), SetVolume(f64), ToggleMute, LiveEdge,
    CycleQuality, CycleCodec, Login, Like,
    PlayUrl(String),
    Play { video_id: String },                    // ← 旧 PlayIndex(usize)。ID 化(§8.2)
    OpenChannel { id: Option<String>, name: String },  // ← 旧 OpenChannelOf(usize)
    OpenList(ListSource), CloseList, ToggleList, ListMove { delta: i32 }, ListSelect, ListBack,
    ToggleChat, ChatScroll(i32), ChatFontBy(f32), SetChatWidth(f32),
    SaveWatchLater { video_id: String }, Feedback { token: String },
    OpenCardMenu(usize), CloseCardMenu,           // メニュー開閉は表示位置の話なので index のまま可
}
fn apply_action(&mut self, a: UiAction) -> bool   // 戻り値 = 「ユーザー操作があった」(last_activity 更新用)
```

3つの入口はすべて **UiAction を組み立てて apply_action を呼ぶだけ**にする:
- `devtools_action(name)` → 文字列を UiAction にパース(名前は既存のまま。外部クライアント互換)
- `window_event` のキーボード → UiAction(グリッド移動は `ListMove { delta: cols }` で従来粒度を維持)
- `about_to_wait` の OverlayAction → `From<OverlayAction> for UiAction`

### 8.2 ID 化(dcomp_overlay 側の小変更)

`OverlayAction::PlayIndex(usize)` は「その瞬間の描画順の座席番号」で、クリックと適用の間に
一覧が更新されると**別の動画を再生する潜在バグ**。オーバーレイは Card 描画時に `card.id`
(video_id)を保持しているので、クリック時にそれを返すよう変更:
`PlayIndex(usize)` → `Play { video_id: String }`、`OpenChannelOf(usize)` → 実 ID/名前、
`SaveWatchLater(usize)`/`NotInterested(usize)` → card.menu が持つ token/id を直接。
再生 URL は `format!("https://www.youtube.com/watch?v={video_id}")` で組めるため、
ID→データの逆引きテーブルは**不要**(YAGNI。必要になったら Content に持たせる)。

### 8.3 ドリフトの統一(このガイドで唯一、挙動を変える指定)

| 対象 | 現状 | 統一後 |
|---|---|---|
| チャット開閉 | オーバーレイ版: `chat_width_ratio` 使用 + 開時 `chat_scroll=0`(931-938)。キーボード/devtools 版: 固定 0.28・scroll 触らず(518-524, 1184-1191) | **オーバーレイ版に統一**(ユーザーが調整した幅を尊重するのが正しい) |
| 一覧の Enter | キーボード版だけ play_list_index を呼ばずインライン再実装(1295-1317) | `ListSelect` → `play_list_index` 相当の単一経路 |

コミットメッセージに挙動変更として明記すること。

### 8.4 Done の定義

- [ ] 同一アクションの実装が grep で1箇所ずつしかない
- [ ] `/action/*` 全名称が従来通り応答(devtools 互換)
- [ ] チャット幅を変えた後、Ctrl+T でも保存済みの幅で開く(ドリフト解消の確認)
- [ ] **ここから先、UI の回帰検証は dev-tools だけで実入力と同等になる**(3系統が同一コードを通るため)

---

## 9. PR U — ui/ 分割(地雷の隔離)

**目的**: native_app.rs を「地雷(OS 境界の暗黙知)」と「普通のコード」に分け、
以後のリファクタが安全に続けられる構造にする。**地雷コードは移動のみ・書き換え禁止**。

### 9.1 分割先

```
src/ui/
  mod.rs
  shell.rs     # NativeApp/NativeRunning 本体、init、window_event の非キーボード系アーム、
               # about_to_wait の可視判定・render 呼び出し・スクショ遅延 = §2.3 の地雷全部
  actions.rs   # UiAction + apply_action + devtools コマンド処理(入力の合流点)
  present.rs   # list_rows / チャット行整形 / auth ラベル / state_json(状態→描画データの純関数)
```

### 9.2 跨ぎ状態の界面(ここだけ設計作業)

地雷側と普通側が共有する可変フィールドは `last_activity` / `overlay_visible` / `focused` /
`pending_shot` / `shot_delay` の5つ。方針:
- 全部 **shell の持ち物**にする
- 普通側(actions)は直接書かず、**「操作があったか」を戻り値で返す**。shell が
  `if state.apply_action(a) { self.last_activity = Instant::now(); }` と一括処理
  (現行 devtools_action の末尾 633-637 が既にこのパターン。全入力経路に一般化するだけ)
- **例外を作らない**こと。Resized で last_activity を更新しない、という地雷の掟(§2.3)は
  「shell 以外は last_activity に触れない」構造そのもので守られる

### 9.3 あわせてやる小物

- native_app.rs 冒頭の古い doc コメント(「egui 版と並存」「現状(骨組み)」)を実態に更新
- §2.3 の地雷3件(hwdec 中の自動非表示 / ドラッグ追従 / フォーカス喪失)を
  「shell 変更時の手動チェックリスト」として docs/ に明文化

### 9.4 Done の定義

- [ ] 共通検証 + 地雷コードの diff が「移動のみ」であること(`git diff --color-moved` で確認)
- [ ] shell.rs 以外に winit の WindowEvent 分岐が存在しない
- [ ] 手動チェック(ユーザーに依頼): 画質切替を連打してもオーバーレイの自動非表示が正常 /
      ウィンドウドラッグ中もオーバーレイが追従 / フォーカスを失ってもチャットが表示されたまま

---

## 10. 詰まったときの処方箋

**借用エラー**
- 「rx を drain しながら self の別メソッドを呼びたい」→ `try_recv()` は値を返した時点で借用が
  切れる(NLL)ので、ループ本体で `&mut self` 系を呼んでよい。旧 poll_auth と同じ構造
- 「poll の結果で別ドメインを触りたい」→ poll に callback を渡さない。**戻り値(bool/イベント)で
  返して、呼び出し側で続ける**(§4.2 の Why)
- 「2つのフィールドを同時に &mut したい」→ 別フィールドなら
  `let (a, b) = (&mut self.account, &mut self.playback);` で分割借用できる。同一フィールド内で
  衝突するなら `Option::take()` で所有権を先に抜く

**可視性エラー**
- Rust の privacy は**子は親に見えるが、親は子の private を見えない**。「Controller(過渡期)から
  content.rs の private フィールドに触れない」と言われたら、直接触るのが間違い —
  system 関数(pub)を経由する。getter を足したくなったら「それは読み取りか?」を確認。
  書き込みの getter(&mut を返す)は絶対ルール2の脱法なので禁止

**Waker まわり**
- closure が `Send + Sync` を満たさないと言われたら、closure 内に `Rc` や非 Send な型を
  捕まえていないか確認。`EventLoopProxy` は Send なので proxy の clone を move すれば通る

**「どこに置くか」で迷ったら**
- そのコード、画面のデザインを変えたら書き直すか? → YES なら bin(ui)
- YouTube の仕様が変わったら書き直すか? → YES なら lib(yt/ またはドメイン)
- 2つ以上のドメインの状態を触るか? → YES なら flows(ただし3本に入らないなら設計相談)
- どちらでもない(OS の挙動の話)→ shell の地雷。触る前にレビュー依頼
