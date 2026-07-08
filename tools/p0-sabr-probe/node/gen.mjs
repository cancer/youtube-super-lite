// PoToken PoC generator (issue #16 案2): WEB visitorData に束ねた PoToken を 1 個生成して JSON 出力する。
// 手段は「何でもよい」ので LuanRT/bgutils-js + jsdom(=BotGuard を DOM 上で実行) を使う（本番は WebView2 想定）。
// 出力: {"visitorData":"...","poToken":"..."}
import { Innertube } from 'youtubei.js';
import { BG } from 'bgutils-js';
import { JSDOM } from 'jsdom';

async function main() {
  // 束ね先(identifier)。引数で渡されればそれ（＝TVHTML5+Bearer セッションの visitorData 等に一致させる）。
  // 無ければ新規 WEB visitorData を採取。
  let visitorData = process.argv[2];
  if (!visitorData) {
    const yt = await Innertube.create({ retrieve_player: false });
    visitorData = yt.session.context.client.visitorData;
  }
  if (!visitorData) throw new Error('visitorData 取得失敗');

  // 2) BotGuard を jsdom 上で実行し、visitorData に束ねた PoToken を生成。
  const requestKey = 'O43z0dpjhgX20SCx4KAo'; // YouTube web の既知 requestKey
  const dom = new JSDOM();
  Object.assign(globalThis, { window: dom.window, document: dom.window.document });

  const bgConfig = {
    fetch: (input, init) => fetch(input, init),
    globalObj: globalThis,
    identifier: visitorData,
    requestKey,
  };

  const challenge = await BG.Challenge.create(bgConfig);
  if (!challenge) throw new Error('challenge 取得失敗');
  const js = challenge.interpreterJavascript.privateDoNotAccessOrElseSafeScriptWrappedValue;
  if (!js) throw new Error('VM スクリプトが無い');
  new Function(js)();

  const result = await BG.PoToken.generate({
    program: challenge.program,
    globalName: challenge.globalName,
    bgConfig,
  });

  process.stdout.write(JSON.stringify({ visitorData, poToken: result.poToken }));
}

main().catch((e) => { console.error('ERR', e?.stack || e); process.exit(1); });
