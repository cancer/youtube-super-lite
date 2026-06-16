# タスク: Rust ネイティブ YouTube 解決器（yt-dlp 置き換え）

> 目的: 再生開始の遅さの主因 = `yt-dlp.exe`(onefile) の毎回 ~3 秒の起動を撤廃する。
> アプリ内（Rust）に常駐させ、URL を渡すだけで再生用ストリームを返す。
> yt-dlp の完全クローンは作らない（このアプリが必要とする機能だけ）。
>
> 前提: yt-dlp を使っているのは **`src/resolve.rs` だけ**（chat/recommend/subs/history/playlist は
> 既に InnerTube 直叩きで yt-dlp 不使用）。置き換え対象は resolve.rs に閉じる。

---

## 必須事項（MUST: これは必ず満たす）

### 契約（呼び出し側を変えない）
- M1. 入力は YouTube URL/ID ＋ `Quality`(Auto/2160/1440/1080/720/480/360) ＋ `Codec`(Auto/H264=avc1/VP9=vp09/AV1=av01)。
- M2. 出力は現行どおり: まず `ResolveUpdate::Ready { video_url, audio_url: Option<String> }` を送り、続いて `Meta { title, is_live }`。
- M3. YouTube 以外の URL は従来どおり素通し（解決処理を通さない）。
- M4. ライブ／VOD／終了ライブのアーカイブ いずれも再生できること（現行の対応範囲を退行させない）。

### 解決処理
- M5. URL→videoId 抽出（watch?v= / youtu.be/ / shorts/ / live/ / nocookie）。
- M6. player レスポンス取得（InnerTube `POST /youtubei/v1/player`、または `ytInitialPlayerResponse`）。
- M7. `videoDetails` から title / is_live を取得して返す（2 回目の yt-dlp 呼び出しを廃止）。
- M8. `streamingData` の formats / adaptiveFormats を解析（itag・mimeType(codec)・height・bitrate・url/signatureCipher・hlsManifestUrl）。
- M9. signature（`signatureCipher` の `s`）を base.js の署名関数で復号して付与。
- M10. **nsig（`n` パラメータ）を base.js の nsig 関数で変換**（未処理だと帯域が絞られ遅くなるため必須）。
- M11. base.js の取得＋署名/nsig 関数の抽出＋JS 実行を行い、**抽出結果と player バージョンをプロセス内キャッシュ**する（＝常駐の肝。2 本目以降はネットワークのみ）。
- M12. フォーマット選択（`build_ytdlp_format` 相当）: Quality は height ≤ 指定、Codec は avc1/vp09/av01 で絞り、Auto は最善。adaptiveFormats から video＋audio を選び mpv へ（audio-file=）。muxed しか無ければそれ。
- M13. ライブは hlsManifestUrl をそのまま mpv に渡す。

### 認証（後から追加された MUST）
- M17. **ログイン状態（OAuth ログイン済み）では members 限定／年齢制限も視聴できること。** 未ログイン時は現行同様に取得不可で可（退行なし）。
  - PoC(U7)で実証済み: **TVHTML5 client に `Authorization: Bearer <access_token>`（既存 scope=youtube.force-ssl）を付けると members/年齢制限が解錠**（streamingData が返る）。cookie/PoToken は不要。
  - ただし TVHTML5 は JS player 必須 → 解錠された URL は `sig`/`n` を持ち、M9(署名)+M10(nsig)処理が無いと 403。**= 認証経路では U5(boa/base.js)が必須**。

### 非機能
- M14. JS 実行は **純 Rust の `boa_engine`** を使う（C 依存を増やさない＝本プロジェクトの方針）。
  - 補足(PoC後): 匿名経路(android_vr/android)は JS 不要。**認証経路(TVHTML5)では nsig変換(M10)が必要**だが、診断(U5)の結果 sig は適用済みのため **M9(署名復号)は実質不要・M10(nsig)のみ**。
  - 実装方針(PoC で確定): 現行 base.js(445213fb)は **VM型(制御フロー平坦化)難読化**で nsig は単独関数として抽出不可。yt-dlp/rustypipe 同様 **base.js 全体をエンジンにロードして実行**し、`.get("n")` の URL 書換関数(`$o6`)を駆動して n を変換する。stubs(window/document/navigator/timers) + raw base.js + descramble ラッパで boa は top-level 実行可(Buffer/atob 不要)。
  - **boa スパイクで実証**: boa_engine 0.20(純Rust)で base.js 全体ロード→nsig が node と全ベクタ一致。コスト=ロード ~6.5s(初回1回・release)/ nsig ~17ms。匿名経路は JS 不要で即時、認証経路の初回だけ遅延ロード・常駐(M15)で吸収。
  - **エンジン交換可能性(設計ルール)**: JS エンジンは狭いトレイト `NsigEngine { load(script); transform_n(n)->String }` の背後に隔離する(boa 型に触れるのは engine_boa.rs だけ)。JS ペイロードはエンジン中立。既定は boa(純Rust,M14)。6.5s ロードが問題化したら rquickjs(QuickJS, パース<1s 想定・C依存)を engine_quickjs.rs として足し feature 切替で差し替え可能(他は無改修)。rustypipe は GPL-3.0 のため不採用。
- M15. 解決器は long-lived（HTTP クライアント・JS エンジン・player キャッシュを保持）にし、アプリ起動時に 1 回だけ初期化。
- M16. 失敗時は現行同様 `ResolveUpdate::Error` を返し、アプリは落とさない。

---

## 禁止事項（MUST NOT: やらない／作らない）

- N1. yt-dlp の**完全クローン化はしない**（汎用サイト対応・全フォーマット網羅をしない）。
- N2. YouTube 以外のサイト対応をしない。
- N3. ダウンロード／多重化（mux）／ファイル保存機能を作らない（再生用 URL の解決のみ）。
- N4. プレイリスト解決・検索・字幕取得・サムネ取得を**このタスクでは**作らない（サムネは自前実装済み・他はスコープ外）。
- N5. C/C++ 依存（quickjs 等のネイティブ JS エンジン）を**第一候補にしない**（M14 のとおり純 Rust 優先）。
- N6. グローバル入力注入や外部ツール常駐（SendKeys 等）に頼らない（検証は dev-tools の HTTP を使う）。
- N7. 既存の呼び出し契約（ResolveUpdate の型・送出順）を壊さない（壊す変更を勝手に入れない）。
- N8. base.js から取り出した関数を**そのまま eval する以外の用途で実行しない**（任意 JS 実行の口を作らない・サンドボックス前提）。

---

## 未決事項（UNDECIDED: 着手前に判断／検証が要る）

- U1. **PoToken / visitorData / bot 対策**（最大の不確実性）: web client は PoToken 無しだと 403/throttle のことがある。**どの client context（web / tv / ios / android / web_embedded）なら token 無しで直リンクが取れるか**を feasibility で確認してから client を確定する。→ 取れない場合の代替（PoToken 生成 or yt-dlp フォールバック維持）も U1 で決める。
- U2. **現行 yt-dlp.exe をフォールバックとして残すか撤去するか**: native が失敗した時に yt-dlp.exe へ自動フォールバックする二段構えにするか、完全撤去するか。
- U3. **DASH→EDL 経路（`build_dash_edl` / dash-mpd）の要否**: adaptiveFormats の直リンクを mpv に直接渡せれば不要になる可能性。終了ライブアーカイブで ranged 取得が mpv で再生できるか検証して決める。
- U4. **base.js 署名/nsig 抽出の保守方針**: YouTube 変更で抽出（正規表現）が壊れる前提。壊れた時に直しやすい構造／テスト動画での自動検知をどこまで用意するか。
- U5. **クレート選定の最終確定**: `boa_engine` で YouTube の nsig 関数（難読化された大きめの JS）が正しく/十分速く動くか要検証。ダメなら代替（U5 で再検討）。
- U6. **対応する client/フォーマットの最小集合**: どの itag/codec まで必ず取れれば良いか（Quality/Codec の全組合せを保証するか、ベストエフォートか）。

---

## 進め方（提案。本実装の前に）
1. **feasibility スパイク**: 1 本の動画で「player 取得 → フォーマット列挙 → 署名/nsig を 1 つ復号 → 直リンクが実際に再生/取得できる」を最小コードで確認。特に **U1(PoToken)** と **U5(boa で nsig 実行)** を見極める。
2. スパイク結果で U1〜U6 を確定 → `resolve.rs` を native 実装へ差し替え（契約 M1〜M3 維持）。
3. ライブ/VOD/終了アーカイブで再生確認（dev-tools 使用）。
