//! YouTube 解決器サイドカー（rustypipe）。
//!
//! 本体（youtube-super-lite）から `resolver-sidecar <videoId>` の形で spawn される。
//! やること:
//!   1. rustypipe で videoId を解決（匿名 bot ゲート突破・署名/nsig は rustypipe が処理）。
//!   2. ローカル中継プロキシ（127.0.0.1）を立て、mpv からの取得を googlevideo へ中継する。
//!   3. stdout に `PROXY_VIDEO=` / `PROXY_AUDIO=` / `READY` を出し、以後は終了されるまで配信し続ける。
//!
//! なぜ中継が要るか（検証で判明した googlevideo の制約）:
//!   - stream URL は「解決した送信元 IP」に固定 → 解決器と mpv が別 IP/ファミリだと 403。
//!     本プロセス内で IPv4 固定の同一クライアントが解決も取得も行い、egress を一致させる。
//!   - 終端なし Range(`bytes=0-`)を拒否 / 1リクエストの最大レンジに上限 / 再生位置からの時間窓スロットリング。
//!     → mpv へは 1 本の 206 として応答しつつ、上流は小さな閉 Range で順次取得し 403 はリトライする。

use reqwest::{Client, ClientBuilder};
use rustypipe::client::RustyPipe;
use rustypipe::model::VideoCodec;
use rustypipe::param::StreamFilter;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// 上流チャンクの初期サイズ。googlevideo は大きすぎるレンジを 403 にするので刻む。
/// 1リクエスト上限はストリームごとに違う（動画は ~10MB 可、音声 opus は ~512KB 等）ため、
/// 403 を受けたら半減して再試行する（下記 serve の適応ロジック）。
const CHUNK_MAX: u64 = 1024 * 1024;
/// 縮小の下限。ここまで縮めても 403 なら「サイズ超過」ではなく時間窓スロットリングと判断し、待って再試行する。
const CHUNK_MIN: u64 = 64 * 1024;
/// チャンク取得の User-Agent（ffmpeg 相当。googlevideo は UA 不問だが明示しておく）。
const FETCH_UA: &str = "Lavf/60.16.100";

struct Streams {
    video: String,
    audio: String,
    video_clen: u64,
    audio_clen: u64,
}

fn clen_of(u: &str) -> u64 {
    u.split(['?', '&'])
        .find_map(|p| p.strip_prefix("clen="))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

#[tokio::main]
async fn main() {
    // 引数: <videoId> [maxRes] [codec]
    //   maxRes: 最大解像度(短辺px)。0 または省略で無制限（= アプリの「自動」）。
    //   codec : auto|h264|vp9|av1（省略は auto）。アプリのユーザー設定をそのまま反映する。
    let id = match std::env::args().nth(1) {
        Some(a) => a,
        None => { eprintln!("usage: resolver-sidecar <videoId> [maxRes] [codec]"); std::process::exit(2); }
    };
    let max_res: u32 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let codec_arg = std::env::args().nth(3).unwrap_or_else(|| "auto".to_string());

    // 解決・取得とも IPv4 egress に固定し、stream URL の ip= ロックと一致させる。
    let client = ClientBuilder::new()
        .local_address(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
        .build()
        .expect("http client");
    let rp_client = ClientBuilder::new().local_address(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    let rp = match RustyPipe::builder().build_with_client(rp_client) {
        Ok(rp) => rp,
        Err(e) => { println!("ERROR=rustypipe build: {e}"); std::process::exit(1); }
    };

    let player = match rp.query().player(&id).await {
        Ok(p) => p,
        Err(e) => { println!("ERROR=resolve: {e}"); std::process::exit(1); }
    };
    let title = player.details.name.as_deref().unwrap_or("").to_string();

    // ライブ配信は HLS マニフェストを mpv が直接扱える（セグメントは mpv/ffmpeg が取得）ため、
    // ローカル中継は不要。HLS URL をそのまま返してプロセスは終了する（常駐プロキシ不要）。
    if player.details.is_live {
        if let Some(hls) = player.hls_manifest_url.as_deref() {
            println!("TITLE={title}");
            println!("IS_LIVE=true");
            println!("PROXY_VIDEO={hls}");
            println!("PROXY_AUDIO=");
            println!("READY");
            use std::io::Write as _;
            let _ = std::io::stdout().flush();
            return;
        }
        // HLS が取れないライブはこのサイドカーでは扱えない（native の HLS 経路に委ねる）。
        println!("ERROR=live without hls manifest");
        std::process::exit(1);
    }

    // アプリのユーザー設定（解像度・コーデック）をそのまま StreamFilter に反映する。
    let mut filter = StreamFilter::default();
    if max_res > 0 {
        filter = filter.video_max_res(max_res);
    }
    filter = match codec_arg.as_str() {
        "h264" => filter.video_codecs([VideoCodec::Avc1]),
        "vp9" => filter.video_codecs([VideoCodec::Vp9]),
        "av1" => filter.video_codecs([VideoCodec::Av01]),
        _ => filter, // auto: コーデック制限なし（rustypipe が最良を選ぶ）
    };
    let (v, a) = player.select_video_audio_stream(&filter);
    let video = v.map(|s| s.url.clone()).unwrap_or_default();
    let audio = a.map(|s| s.url.clone()).unwrap_or_default();
    if video.is_empty() {
        println!("ERROR=no video stream");
        std::process::exit(1);
    }
    let streams = Arc::new(Streams {
        video_clen: clen_of(&video),
        audio_clen: clen_of(&audio),
        video,
        audio,
    });

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().unwrap().port();

    println!("TITLE={title}");
    println!("IS_LIVE={}", player.details.is_live);
    println!("PROXY_VIDEO=http://127.0.0.1:{port}/v");
    if streams.audio.is_empty() {
        println!("PROXY_AUDIO=");
    } else {
        println!("PROXY_AUDIO=http://127.0.0.1:{port}/a");
    }
    println!("READY");
    use std::io::Write as _;
    let _ = std::io::stdout().flush();

    loop {
        let (sock, _) = match listener.accept().await { Ok(x) => x, Err(_) => continue };
        let streams = streams.clone();
        let client = client.clone();
        tokio::spawn(async move { let _ = serve(sock, streams, client).await; });
    }
}

/// mpv からの 1 リクエストを処理する。要求区間を 1 本の 206 として返しつつ、
/// 上流は閉 Range の小チャンクで順次取得（時間窓 403 はリトライ）して中継する。
async fn serve(mut sock: TcpStream, streams: Arc<Streams>, client: Client) -> std::io::Result<()> {
    // リクエストヘッダを \r\n\r\n まで読む。
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        let n = sock.read(&mut tmp).await?;
        if n == 0 { return Ok(()); }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 65536 { break; }
    }
    let req = String::from_utf8_lossy(&buf);
    let mut lines = req.split("\r\n");
    let reqline = lines.next().unwrap_or("");
    let path = reqline.split_whitespace().nth(1).unwrap_or("/");
    let mut range = None;
    for l in lines {
        if l.len() >= 6 && l[..6].eq_ignore_ascii_case("range:") {
            range = Some(l[6..].trim().to_string());
        }
    }

    let is_audio = path.starts_with("/a");
    let url = if is_audio { &streams.audio } else { &streams.video };
    let clen = if is_audio { streams.audio_clen } else { streams.video_clen };
    if url.is_empty() {
        let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
        return Ok(());
    }

    let (start, end) = parse_range(&range, clen);
    let total = end - start + 1;
    let ctype = if is_audio { "audio/webm" } else { "video/mp4" };
    let head = format!(
        "HTTP/1.1 206 Partial Content\r\nContent-Type: {ctype}\r\nAccept-Ranges: bytes\r\n\
         Content-Length: {total}\r\nContent-Range: bytes {start}-{end}/{clen}\r\nConnection: close\r\n\r\n"
    );
    sock.write_all(head.as_bytes()).await?;

    let mut size = CHUNK_MAX;
    let mut pos = start;
    'outer: while pos <= end {
        let mut tries = 0u32;
        let (resp, chunk_end) = loop {
            let chunk_end = (pos + size - 1).min(end);
            let rb = client.get(url).header("User-Agent", FETCH_UA)
                .header("Range", format!("bytes={pos}-{chunk_end}"));
            match rb.send().await {
                Ok(r) if r.status().is_success() => break (r, chunk_end),
                // 403 等。まずはサイズ超過を疑い下限まで半減して即再試行（音声 opus は ~512KB 上限など）。
                Ok(_) if size > CHUNK_MIN => size /= 2,
                // 下限でも 403 = 時間窓スロットリング。窓が前進するのを待って再試行。
                Ok(_) => {
                    tries += 1;
                    if tries > 60 {
                        break 'outer;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                Err(_) => break 'outer,
            }
        };
        let mut body = resp.bytes_stream();
        while let Some(chunk) = body.next().await {
            match chunk {
                Ok(b) => { if sock.write_all(&b).await.is_err() { break 'outer; } }
                Err(_) => break 'outer,
            }
        }
        pos = chunk_end + 1;
    }
    Ok(())
}

/// mpv の Range から (start, end) を返す。終端未指定は clen-1（＝全体）。
fn parse_range(range: &Option<String>, clen: u64) -> (u64, u64) {
    let last = clen.saturating_sub(1);
    let spec = match range {
        Some(r) => r.trim().strip_prefix("bytes=").unwrap_or("0-").to_string(),
        None => "0-".to_string(),
    };
    let mut it = spec.splitn(2, '-');
    let start = it.next().unwrap_or("0").parse::<u64>().unwrap_or(0);
    let end = it.next().unwrap_or("").parse::<u64>().unwrap_or(last);
    (start, end.min(last).max(start))
}
