import { describe, expect, test } from "bun:test";

import { installIntercept } from "../src/main/intercept";

const WATCH_URL = "https://www.youtube.com/youtubei/v1/get_watch?prettyPrint=false";
/** 変換対象でない URL の代表。実測で fetch を経由することが分かっているメディアセグメント。 */
const MEDIA_URL = "https://rr3---sn-x.googlevideo.com/videoplayback?itag=140";

const XHR_DONE = 4;
const XHR_LOADING = 3;

/**
 * XMLHttpRequest の最小の代役。
 *
 * bun のテスト環境に XMLHttpRequest が無いため用意する。パッチはプロトタイプに当たるので、
 * テストごとに別のクラスを作って他のテストへ影響が漏れないようにしてある。
 */
const createXhrClass = () =>
  class FakeXhr {
    readyState = 0;
    responseType: XMLHttpRequestResponseType = "";
    private body = "";

    open(_method: string, _url: string | URL): void {
      this.readyState = 1;
      this.body = "";
    }

    /** サーバ応答の到着をテストから再現する。 */
    receive(body: string, readyState: number = XHR_DONE): void {
      this.body = body;
      this.readyState = readyState;
    }

    get responseText(): string {
      return this.body;
    }

    get response(): unknown {
      return this.responseType === "json" ? JSON.parse(this.body) : this.body;
    }
  };

const jsonResponse = (value: unknown): Response =>
  new Response(JSON.stringify(value), {
    headers: { "content-type": "application/json" },
  });

const urlOf = (input: RequestInfo | URL): string =>
  typeof input === "string" ? input : input instanceof URL ? input.href : input.url;

const makeGlobals = (respond: (url: string) => Response = jsonResponse) => ({
  fetch: (input: RequestInfo | URL): Promise<Response> =>
    Promise.resolve(respond(urlOf(input))),
  XMLHttpRequest: createXhrClass(),
});

describe("fetch の傍受", () => {
  // 最重要。全レスポンスの body を読む実装にするとメディアストリームをバッファリングし、
  // 「長時間視聴での定常メモリ増加」という最優先の評価軸を実装ミスで壊す。
  test("変換対象でない URL では元の Response を同一参照で返す", async () => {
    const original = jsonResponse({ segment: true });
    const globals = makeGlobals(() => original);
    installIntercept(globals).register("watch", () => ({ replaced: true }));

    expect(await globals.fetch(MEDIA_URL)).toBe(original);
  });

  test("変換対象でない URL では body を読まない", async () => {
    const original = jsonResponse({ segment: true });
    const globals = makeGlobals(() => original);
    installIntercept(globals).register("watch", () => ({ replaced: true }));

    await globals.fetch(MEDIA_URL);

    expect(original.bodyUsed).toBe(false);
  });

  test("変換対象の URL では変換後の JSON をページに渡す", async () => {
    const globals = makeGlobals(() => jsonResponse({ keep: 1, drop: 2 }));
    installIntercept(globals).register("watch", (json) => ({
      keep: (json as { keep: number }).keep,
    }));

    expect(await (await globals.fetch(WATCH_URL)).json()).toEqual({ keep: 1 });
  });

  test("変換対象だが変換関数が未登録なら元の Response を同一参照で返す", async () => {
    const original = jsonResponse({ untouched: true });
    const globals = makeGlobals(() => original);
    installIntercept(globals);

    expect(await globals.fetch(WATCH_URL)).toBe(original);
  });

  test("変換関数が throw したら元の Response を素通しする", async () => {
    const original = jsonResponse({ intact: true });
    const globals = makeGlobals(() => original);
    installIntercept(globals).register("watch", () => {
      throw new Error("スキーマ変更を模した失敗");
    });

    const response = await globals.fetch(WATCH_URL);

    expect(response).toBe(original);
    expect(await response.json()).toEqual({ intact: true });
  });

  test("変換対象の応答が JSON でなければ元の Response を素通しする", async () => {
    const original = new Response("<html>error</html>");
    const globals = makeGlobals(() => original);
    installIntercept(globals).register("watch", () => ({ replaced: true }));

    expect(await globals.fetch(WATCH_URL)).toBe(original);
  });

  test("変換後の Response に元の content-length を引き継がない", async () => {
    const globals = makeGlobals(
      () =>
        new Response(JSON.stringify({ keep: 1, drop: 2 }), {
          headers: { "content-length": "24", "content-type": "application/json" },
        }),
    );
    installIntercept(globals).register("watch", () => ({ keep: 1 }));

    const response = await globals.fetch(WATCH_URL);

    expect(response.headers.get("content-length")).toBeNull();
  });

  test("変換後の Response が元のステータスを引き継ぐ", async () => {
    const globals = makeGlobals(
      () => new Response(JSON.stringify({ a: 1 }), { status: 201, statusText: "Created" }),
    );
    installIntercept(globals).register("watch", (json) => json);

    expect((await globals.fetch(WATCH_URL)).status).toBe(201);
  });
});

describe("XMLHttpRequest の傍受", () => {
  test("変換対象でない URL では responseText を変換しない", () => {
    const globals = makeGlobals();
    installIntercept(globals).register("watch", () => ({ replaced: true }));

    const xhr = new globals.XMLHttpRequest();
    xhr.open("POST", "https://www.youtube.com/api/stats/watchtime?ns=yt");
    xhr.receive('{"kept":true}');

    expect(xhr.responseText).toBe('{"kept":true}');
  });

  test("変換対象の URL では responseText が変換後の JSON になる", () => {
    const globals = makeGlobals();
    installIntercept(globals).register("live_chat", () => ({ trimmed: true }));

    const xhr = new globals.XMLHttpRequest();
    xhr.open("POST", "https://www.youtube.com/youtubei/v1/live_chat/get_live_chat");
    xhr.receive('{"actions":[1,2,3]}');

    expect(JSON.parse(xhr.responseText)).toEqual({ trimmed: true });
  });

  test("変換関数が throw したら responseText を元のまま返す", () => {
    const globals = makeGlobals();
    installIntercept(globals).register("live_chat", () => {
      throw new Error("スキーマ変更を模した失敗");
    });

    const xhr = new globals.XMLHttpRequest();
    xhr.open("POST", "https://www.youtube.com/youtubei/v1/live_chat/get_live_chat");
    xhr.receive('{"actions":[1,2,3]}');

    expect(xhr.responseText).toBe('{"actions":[1,2,3]}');
  });

  test("responseType が json のとき response が変換後のオブジェクトになる", () => {
    const globals = makeGlobals();
    installIntercept(globals).register("live_chat", () => ({ trimmed: true }));

    const xhr = new globals.XMLHttpRequest();
    xhr.responseType = "json";
    xhr.open("POST", "https://www.youtube.com/youtubei/v1/live_chat/get_live_chat");
    xhr.receive('{"actions":[1,2,3]}');

    expect(xhr.response).toEqual({ trimmed: true });
  });

  test("完了前の responseText は変換しない", () => {
    const globals = makeGlobals();
    installIntercept(globals).register("live_chat", () => ({ trimmed: true }));

    const xhr = new globals.XMLHttpRequest();
    xhr.open("POST", "https://www.youtube.com/youtubei/v1/live_chat/get_live_chat");
    xhr.receive('{"actions":[1,2', XHR_LOADING);

    expect(xhr.responseText).toBe('{"actions":[1,2');
  });

  test("同じインスタンスを変換対象でない URL に開き直したら変換しない", () => {
    const globals = makeGlobals();
    installIntercept(globals).register("live_chat", () => ({ trimmed: true }));

    const xhr = new globals.XMLHttpRequest();
    xhr.open("POST", "https://www.youtube.com/youtubei/v1/live_chat/get_live_chat");
    xhr.receive('{"actions":[1]}');
    xhr.open("POST", "https://www.youtube.com/api/stats/watchtime?ns=yt");
    xhr.receive('{"kept":true}');

    expect(xhr.responseText).toBe('{"kept":true}');
  });
});

describe("多重注入への耐性", () => {
  test("二重に install しても fetch を二重にパッチしない", () => {
    const globals = makeGlobals();
    installIntercept(globals);
    const patched = globals.fetch;

    installIntercept(globals);

    expect(globals.fetch).toBe(patched);
  });

  test("二重に install しても open を二重にパッチしない", () => {
    const globals = makeGlobals();
    installIntercept(globals);
    const patched = globals.XMLHttpRequest.prototype.open;

    installIntercept(globals);

    expect(globals.XMLHttpRequest.prototype.open).toBe(patched);
  });

  test("二重に install しても先に登録した変換が効き続ける", async () => {
    const globals = makeGlobals(() => jsonResponse({ keep: 1, drop: 2 }));
    installIntercept(globals).register("watch", () => ({ keep: 1 }));

    installIntercept(globals);

    expect(await (await globals.fetch(WATCH_URL)).json()).toEqual({ keep: 1 });
  });
});
