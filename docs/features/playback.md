# 動画再生

対象読者: 再生できる動画の種類・画質/コーデック切替・シーク挙動を確認したい人。

## 概要

| 項目 | 内容 |
|------|------|
| 対応コンテンツ | 短尺動画・配信中ライブ・終了ライブのアーカイブ |
| 認証 | 不要（公開動画）。メンバー限定・年齢制限動画はログインが必要 |
| 再生エンジン | libmpv（`vo=gpu-next` `gpu-api=d3d11`。mpv 自身がウィンドウへ D3D11 で直接描画） |
| URL解決 | アプリ内蔵のネイティブ InnerTube リゾルバ（詳細: [design/url-resolution.md](../design/url-resolution.md)） |

## 画質・コーデック選択

`Ctrl+Q`（画質）/ `Ctrl+C`（コーデック）で切り替える。画質は 自動 / 2160p / 1440p / 1080p / 720p / 480p / 360p、
コーデックは 自動 / H.264 / VP9 / AV1 から選ぶ。切り替えると現在の再生位置を保ったまま該当フォーマットで
取り直す。実際にどのフォーマットが選ばれるかは [design/url-resolution.md#フォーマット選択](../design/url-resolution.md) を参照。

## GPU負荷に応じた自動デコード切替

GPU 使用率が高い状態が続くとソフトウェアデコードへ、落ち着くとハードウェアデコードへ自動的に戻る
（ユーザー操作は不要）。詳細は [ui-settings-and-gpu.md](ui-settings-and-gpu.md) を参照。

## シークとキャッシュ

mpv 初期化時に以下を設定し、DASH の adaptive ストリーム（映像+別音声の2ストリーム）でもシークの
再取得コストを抑えている:

- `cache=yes` / `demuxer-seekable-cache=yes`: シーク済み範囲を破棄せず再利用
- `demuxer-max-bytes=256MiB` / `demuxer-max-back-bytes=128MiB`: 前後方向のキャッシュ上限

キャッシュ内へのシークはほぼ即時。未バッファの前方への大ジャンプは、映像・音声2本ぶんの range
リクエストを再取得するため数秒かかることがある（本質的なコストで、ハングはしない）。

## DASH（終了ライブアーカイブ等）

ffmpeg 標準ビルドは DASH demuxer 非対応のため、`dash-mpd` クレートで MPD をパースして mpv の EDL に
変換して再生する。設計の詳細は [design/dash-playback.md](../design/dash-playback.md)。

## 既知の制限

- フルスクリーン切替は未実装
- macOS 未対応（D3D11 前提。Metal + CoreAnimation 対応が今後の課題）

## 関連

- [controller-ui.md](controller-ui.md) — コントローラ帯の表示・操作
- [devtools.md](devtools.md) — `--enable-dev-tools` からの再生操作
