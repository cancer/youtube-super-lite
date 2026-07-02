# DASH manifest 対応の設計

対象読者: 終了ライブのアーカイブなど DASH でしか配信されないコンテンツの再生ロジックを追う人。

## 課題

ffmpeg（標準ビルド）は DASH demuxer に対応していない。YouTube は動画の種類によっては DASH manifest しか
返さないことがあり（配信終了後のライブアーカイブなど）、これを ffmpeg/mpv にそのまま渡しても再生できない。

## 解決方法

1. [ネイティブリゾルバ](url-resolution.md)（またはその sidecar フォールバック）でストリーム URL を取得する
2. URL が DASH manifest（`manifest.googlevideo.com/api/manifest/dash/...`）と判定されたら、
   **`dash-mpd`** クレートで MPD XML をパースする
3. `SegmentTemplate` の `$Number$` 等のプレースホルダを展開し、各セグメントの実URLを生成する
4. mpv の EDL（Edit Decision List、`edl://!mp4_dash,init=...;seg1;seg2;...`）としてセグメント列を組み立て、
   `loadfile` に渡す

mpv 自体は EDL 形式であれば普通の入力として扱えるため、ffmpeg の DASH demuxer 非対応を EDL 経由で
迂回する形になる。これにより配信中ライブ・終了ライブのアーカイブ・短尺動画のいずれも同じ再生パスで扱える。

## 関連

- [url-resolution.md](url-resolution.md) — DASH URL の判定より前段にあるストリームURL取得
- [../features/playback.md](../features/playback.md) — ユーザーから見た再生対象の種類
