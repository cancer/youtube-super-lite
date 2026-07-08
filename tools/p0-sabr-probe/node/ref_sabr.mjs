// 参照実装(@luanrt/googlevideo)で TVHTML5+Bearer live の SABR を叩く（reviewer 提案 b）。
// 目的: 私の Rust probe の 403 が「私の実装バグ(protobuf/n)」か「YouTube 側の壁」かを切り分ける。
// reference の protobuf encoder + youtubei.js の n-decipher + reference UMP reader を使う。
// 使い方: node ref_sabr.mjs <liveVideoId> [--pot]
import { readFileSync } from 'fs';
import { homedir } from 'os';
import { join } from 'path';
import { Innertube, Platform } from 'youtubei.js';
import { VideoPlaybackAbrRequest } from 'googlevideo/protos';
import { UmpReader, CompositeBuffer } from 'googlevideo/ump';
import { base64ToU8 } from 'googlevideo/utils';
import { BG } from 'bgutils-js';
import { JSDOM } from 'jsdom';

const BACKEND = 'https://youtube-super-lite-backend.cancer6.workers.dev';
const TV = { name: 'TVHTML5', version: '7.20260114.12.00', id: 7, ua: 'Mozilla/5.0 (ChromiumStylePlatform) Cobalt/Version' };

const videoId = process.argv[2] || 'gCNeDWCI0vo';
const withPot = process.argv.includes('--pot');
const webMode = process.argv.includes('--web'); // WEB 匿名+pot（reference downloader の実働構成）
const WEB = { name: 'WEB', version: '2.20260114.08.00', id: 1, ua: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36' };

// youtubei.js 17 は decipher に自前 JS evaluator を要求する（LuanRT の例と同じ shim）。
Platform.shim.eval = async (data, env) => {
  const props = [];
  if (env.n) props.push(`n: exportedVars.nFunction(${JSON.stringify(env.n)})`);
  if (env.sig) props.push(`sig: exportedVars.sigFunction(${JSON.stringify(env.sig)})`);
  const code = `${data.output}\nreturn { ${props.join(', ')} }`;
  return new Function(code)();
};

async function accessToken() {
  const p = join(process.env.APPDATA || homedir(), 'YouTubeSuperLite', 'auth.json');
  const refresh = JSON.parse(readFileSync(p, 'utf8')).refresh_token;
  const r = await fetch(`${BACKEND}/refresh`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ refresh_token: refresh }) });
  const j = await r.json();
  if (!j.access_token) throw new Error('no access_token: ' + JSON.stringify(j));
  return j.access_token;
}

async function genPoToken(visitorData) {
  const dom = new JSDOM();
  Object.assign(globalThis, { window: dom.window, document: dom.window.document });
  const bgConfig = { fetch: (i, o) => fetch(i, o), globalObj: globalThis, identifier: visitorData, requestKey: 'O43z0dpjhgX20SCx4KAo' };
  const ch = await BG.Challenge.create(bgConfig);
  new Function(ch.interpreterJavascript.privateDoNotAccessOrElseSafeScriptWrappedValue)();
  const res = await BG.PoToken.generate({ program: ch.program, globalName: ch.globalName, bgConfig });
  return res.poToken;
}

async function player(cl, token, visitorData, poToken) {
  const client = { clientName: cl.name, clientVersion: cl.version, hl: 'en', gl: 'US' };
  if (visitorData) client.visitorData = visitorData;
  const body = { context: { client }, videoId, contentCheckOk: true, racyCheckOk: true };
  if (poToken) body.serviceIntegrityDimensions = { poToken };
  const headers = {
    'content-type': 'application/json', 'user-agent': cl.ua,
    'x-youtube-client-name': String(cl.id), 'x-youtube-client-version': cl.version,
    origin: 'https://www.youtube.com',
  };
  if (token) headers.authorization = `Bearer ${token}`;
  if (visitorData) headers['x-goog-visitor-id'] = visitorData;
  const r = await fetch('https://www.youtube.com/youtubei/v1/player?prettyPrint=false', { method: 'POST', headers, body: JSON.stringify(body) });
  return r.json();
}

function pickFmt(list, kind) {
  const fs = list.filter(f => (f.mimeType || '').startsWith(kind + '/'));
  fs.sort((a, b) => kind === 'video' ? (b.height || 0) - (a.height || 0) : (b.bitrate || 0) - (a.bitrate || 0));
  const f = fs[0];
  return f && { itag: f.itag, lastModified: f.lastModified, xtags: f.xtags };
}

async function main() {
  const cl = webMode ? WEB : TV;
  const usePot = withPot || webMode; // WEB は匿名なので pot 前提
  console.log(`=== ref SABR (googlevideo) client=${cl.name}${webMode ? '(anon)' : '+Bearer'} id=${videoId} pot=${usePot} ===`);
  const token = webMode ? null : await accessToken();
  if (token) console.log('access_token OK');

  // youtubei.js の player を n-decipher に使う。
  const yt = await Innertube.create({ retrieve_player: true });
  console.log('player_id(base.js) =', yt.session.player?.player_id);

  // pot を使う場合、先に visitorData+pot を発行して player 要求にも載せる。
  let visitorData, poToken;
  if (usePot) {
    const dom = await Innertube.create({ retrieve_player: false });
    visitorData = dom.session.context.client.visitorData;
    poToken = await genPoToken(visitorData);
    console.log('poToken 発行 OK', poToken.length, '文字 / visitorData', visitorData.length);
  }

  const resp = await player(cl, token, visitorData, poToken);
  console.log('playabilityStatus =', resp.playabilityStatus?.status, '/ is_live =', !!resp.videoDetails?.isLive);
  const sd = resp.streamingData;
  if (!sd?.serverAbrStreamingUrl) { console.log('no serverAbrStreamingUrl (hls?', !!sd?.hlsManifestUrl, ')'); return; }
  const ustreamer = resp.playerConfig?.mediaCommonConfig?.mediaUstreamerRequestConfig?.videoPlaybackUstreamerConfig;
  const video = pickFmt(sd.adaptiveFormats, 'video');
  const audio = pickFmt(sd.adaptiveFormats, 'audio');
  console.log('video', JSON.stringify(video), 'audio', JSON.stringify(audio));

  // n を youtubei.js の player で正しく変換。
  const abrUrl = await yt.session.player.decipher(sd.serverAbrStreamingUrl);
  const nOrig = new URL(sd.serverAbrStreamingUrl).searchParams.get('n');
  const nNew = new URL(abrUrl).searchParams.get('n');
  console.log('n:', nOrig, '→', nNew, nOrig === nNew ? '(未変化!)' : '(変化)');

  // reference の protobuf encoder で本体を作る。
  const req = {
    clientAbrState: { playerTimeMs: '0', enabledTrackTypesBitfield: 0 },
    selectedFormatIds: [],
    bufferedRanges: [],
    preferredAudioFormatIds: audio ? [audio] : [],
    preferredVideoFormatIds: video ? [video] : [],
    preferredSubtitleFormatIds: [],
    videoPlaybackUstreamerConfig: base64ToU8(ustreamer),
    streamerContext: {
      clientInfo: { clientName: cl.id, clientVersion: cl.version },
      poToken: poToken ? base64ToU8(poToken) : undefined,
      sabrContexts: [], unsentSabrContexts: [],
    },
    field1000: [],
  };
  const encoded = VideoPlaybackAbrRequest.encode(req).finish();
  console.log('protobuf', encoded.length, 'bytes');

  const postHeaders = { 'content-type': 'application/x-protobuf', 'user-agent': cl.ua, origin: 'https://www.youtube.com', 'accept-encoding': 'identity' };
  if (token) postHeaders.authorization = `Bearer ${token}`;
  const r = await fetch(abrUrl, { method: 'POST', headers: postHeaders, body: encoded });
  console.log('POST status =', r.status, r.headers.get('content-type'));
  const buf = new Uint8Array(await r.arrayBuffer());
  console.log('response', buf.length, 'bytes');

  // reference UMP reader でパート集計。
  if (buf.length) {
    const reader = new UmpReader(new CompositeBuffer([buf]));
    const counts = {};
    let media = 0;
    reader.read(part => { counts[part.type] = (counts[part.type] || 0) + part.size; if (part.type === 21) media += part.size; });
    console.log('UMP parts (type:bytes):', JSON.stringify(counts));
    console.log(media > 0 ? `✅ MEDIA ${media} bytes 受信` : '❌ メディア 0 bytes');
  }
}
main().catch(e => { console.error('ERR', e?.stack || e); process.exit(1); });
