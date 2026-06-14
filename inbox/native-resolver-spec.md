# タスク: Rust ネイティブ YouTube 解決器（yt-dlp 置き換え）

> 目的: 再生開始の遅さの主因 = `yt-dlp.exe`(onefile) の毎回 ~3 秒の起動を撤廃する。
> アプリ内（Rust）で常駐し、URL を渡すだけで再生用ストリームを返す。完全な yt-dlp クローンは作らない。
> 必要なのは「このアプリが yt-dlp に求めている機能」だけ。

## スコープの大前提
- yt-dlp を使っているのは **`src/resolve.rs` だけ**（chat/recommend/subs/history/playlist は既に InnerTube 直叩きで yt-dlp 不使用）。
- 既存の契約（呼び出し側を変えない）を維持する:
  - 入力: YouTube URL/ID＋`Quality`(Auto/2160/1440/1080/720/480/360)＋`Codec`(Auto/H264=avc1/VP9=vp09/AV1=av01)
  - 出力: `ResolveUpdate::Ready { video_url, audio_url: Option<String> }` → 後追いで `Meta { title, is_live }`
  - YouTube 以外の URL は従来どおり素通し（解決不要）

## 必須機能（MUST）
1. **URL → videoId 抽出**（watch?v= / youtu.be/ / shorts / nocookie / live/）。既存 `auth::extract_video_id` を拡張。
2. **player レスポンス取得**: InnerTube `POST /youtubei/v1/player`（client context 付き）またはウォッチページの `ytInitialPlayerResponse`。
   - INNERTUBE_API_KEY / clientName / clientVersion / visitorData の取得（chat.rs の抽出パターンを流用）。
3. **streamingData / videoDetails パース**:
   - `videoDetails`: title, isLive / isLiveContent, lengthSeconds
   - `formats`(muxed) + `adaptiveFormats`(分離): itag, mimeType(codec+container), height, bitrate, audioQuality, `url` か `signatureCipher`, contentLength
   - `hlsManifestUrl`(ライブ), `dashManifestUrl`
4. **署名（signature）処理**: `signatureCipher`(s/sp/url) の `s` を base.js の署名関数で変換して付与。
5. **nsig（n パラメータ）処理**: URL の `n=` を base.js の nsig 関数で変換。**未処理だと帯域が絞られ再生が遅くなる**ため必須。
   - → **base.js の取得＋署名/nsig 関数の抽出（正規表現）＋ JS 実行**が必要。
   - JS 実行エンジン: **`boa_engine`（純 Rust・C 依存なし＝本プロジェクト方針に合致）** を第一候補（代替: rquickjs）。
   - 抽出した関数と player バージョンを**プロセス内キャッシュ**（常駐の肝。2 本目以降はネットワークのみ）。
6. **フォーマット選択**（`build_ytdlp_format` 相当を Rust で）:
   - Quality: height ≤ 指定。Codec: avc1/vp09/av01 で絞る。Auto は最善。
   - adaptiveFormats から best video＋best audio を選び、`video_url` と `audio_url` を返す（mpv に audio-file= で渡す）。muxed しか無ければそれ。
7. **ライブ対応**: `hlsManifestUrl` をそのまま mpv に渡す（mpv が HLS 再生）。`is_live=true`。
8. **title / is_live を返す**（videoDetails から。yt-dlp の 2 回呼び出しが不要になる）。
9. **常駐**: 解決器を long-lived（HTTP クライアント・JS エンジン・player キャッシュを保持）にしてアプリ起動時に 1 回だけ初期化。

## 調査が必要（要検証・リスク）
- **PoToken / visitorData / bot 対策**: 近年の YouTube は web client で PoToken を要求し、無しだと 403 や throttling のことがある。どの client context（web / tv / ios / android）なら token 無しで直リンクが取れるか要検証。**ここが最大の不確実性**。
- **base.js 署名/nsig 抽出の追従性**: YouTube 変更で抽出正規表現が壊れる（yt-dlp が頻繁に更新している部分）。壊れた時に直しやすい構造にする。
- **既存 DASH→EDL 経路の要否**: adaptiveFormats の直リンクを mpv に直接渡せれば、現在の `build_dash_edl`（dash-mpd で manifest をセグメント展開）は不要になる可能性。終了ライブアーカイブで直リンクが ranged 取得で再生できるか要検証（ダメなら EDL 経路を残す）。

## やらないこと（非スコープ）
- プレイリスト/検索/字幕/サムネ取得（サムネは自前実装済み）
- YouTube 以外のサイト、ダウンロード/マージ、全フォーマット網羅
- 完全な yt-dlp 互換

## 追加クレート（想定）
- `boa_engine`（署名/nsig の JS 実行・純 Rust）
- `regex`（base.js からの関数抽出）
- （既存: reqwest / serde_json を流用）

## 進め方（提案）
1. **スパイク（feasibility）**: 1 本の動画で player レスポンス取得 → フォーマット列挙 → 署名/nsig を 1 つ復号 → 直リンクが実際に再生（または HTTP 200 で取得）できるかを最小コードで確認。特に PoToken 要否を見極める。
2. 行けるなら `resolve.rs` を native 実装に差し替え（契約は維持）。yt-dlp.exe はフォールバックとして残すか撤去するか判断。
3. ライブ/VOD/終了アーカイブで再生確認（dev-tools 使用）。

## メモ
- 成功すれば TTFF の ~3 秒（yt-dlp 起動）が消え、抽出はプロセス内常駐でネットワーク分のみ＝本家に近づく。
- まずは 1 の feasibility スパイクで PoToken/署名の壁を確認してから本実装に入るのが安全。
