import { transformTargetOf, type TransformTarget } from "../shared/endpoints";

/**
 * fetch と XMLHttpRequest の共通傍受層。
 *
 * R2（コメント・関連動画の除去）と R3（ライブチャット軽量化）はどちらも
 * 「特定のエンドポイントの JSON 応答を書き換えてページに渡す」形で成立する。
 * 差し替えの機構をここに 1 つだけ置き、機能側は変換関数を登録するだけにする。
 *
 * 守る不変条件は 2 つ。
 * 1. 変換対象でない応答の body には一切触れず、元の Response を同一参照で返す。
 *    メディアセグメントも fetch を経由するため、全応答の body を読む実装にすると
 *    ストリームをバッファリングし、最優先の評価軸（長時間視聴での定常メモリ増加）を壊す。
 * 2. 変換の失敗はページに波及させない。失敗時は元の応答を素通しし、
 *    「軽量化が効かなくなるだけ」に落とす。
 */

/** JSON 応答を受け取り、ページへ渡す JSON を返す。R2 / R3 が実装する。 */
export type JsonTransform = (json: unknown) => unknown;

/**
 * 変換関数の登録口。
 *
 * 1 つの target に対して変換は 1 つ（watch は R2、live_chat は R3 が持つ）。
 * 同じ target への再登録は上書きになる。
 */
export type TransformRegistry = {
  register: (target: TransformTarget, transform: JsonTransform) => void;
};

type FetchLike = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

/** XMLHttpRequest のうち、傍受層が触る部分だけを表した型。 */
type PatchableXhr = {
  readonly readyState: number;
  readonly responseType: XMLHttpRequestResponseType;
  readonly responseText: string;
  readonly response: unknown;
};

type XhrOpen = (method: string, url: string | URL, ...rest: unknown[]) => void;

type PatchableXhrConstructor = {
  prototype: PatchableXhr & { open: XhrOpen };
};

/** パッチ対象。ブラウザでは globalThis を渡す。 */
export type InterceptTarget = {
  fetch: FetchLike;
  XMLHttpRequest: PatchableXhrConstructor;
};

type InterceptState = {
  transforms: Map<TransformTarget, JsonTransform>;
};

/**
 * パッチ済みかどうかのマーカー兼、登録済み変換の置き場。
 *
 * globalThis ではなくパッチ対象そのものに載せる。マーカーが表しているのは
 * 「この対象をパッチしたか」なので、対象と同じ寿命を持たせるのが正しい。
 */
const STATE_KEY = "__youtubeSuperLiteIntercept";

type MarkedTarget = InterceptTarget & { [STATE_KEY]?: InterceptState };

const XHR_DONE = 4;

/** 変換の失敗を吸収する。スキーマ変更や変換のバグでページを壊さないため。 */
const safeTransform = (json: unknown, transform: JsonTransform): unknown => {
  try {
    return transform(json);
  } catch {
    return json;
  }
};

/** JSON 文字列を変換する。パースも変換も失敗したら元の文字列を返す。 */
const safeTransformText = (raw: string, transform: JsonTransform): string => {
  try {
    return JSON.stringify(transform(JSON.parse(raw) as unknown));
  } catch {
    return raw;
  }
};

const urlStringOf = (input: RequestInfo | URL): string =>
  typeof input === "string" ? input : input instanceof URL ? input.href : input.url;

const transformFor = (
  state: InterceptState,
  url: string,
): JsonTransform | undefined => {
  const target = transformTargetOf(url);
  return target === undefined ? undefined : state.transforms.get(target);
};

const transformResponse = async (
  response: Response,
  transform: JsonTransform,
): Promise<Response> => {
  let body: string;
  try {
    // clone してから読む。変換や JSON パースが失敗したときに、body 未読のままの
    // 元の Response をそのまま返せるようにするため。
    body = JSON.stringify(transform(await response.clone().json()));
  } catch {
    return response;
  }

  const headers = new Headers(response.headers);
  // 変換でバイト長が変わるため、元の値を引き継ぐと本文と食い違う。
  headers.delete("content-length");
  return new Response(body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  });
};

const patchFetch = (target: InterceptTarget, state: InterceptState): void => {
  const originalFetch = target.fetch;
  target.fetch = (input, init) => {
    const transform = transformFor(state, urlStringOf(input));
    // 非対象は元の Promise をそのまま返す。await を挟まないので、高頻度で走る
    // メディアセグメントの取得に余計なラッパも body の読み取りも積まない。
    if (transform === undefined) return originalFetch(input, init);
    return originalFetch(input, init).then((response) =>
      transformResponse(response, transform),
    );
  };
};

/**
 * 変換対象として開かれた応答に紐づく状態。
 *
 * `transformed` は responseText 用の記憶で、同じ応答を何度読まれても文字列の変換とパースは
 * 1 回に留める。responseType: "json" の経路にはこれに当たる記憶を持たない（下の responseOf）。
 */
type XhrPending = { transform: JsonTransform; transformed?: string };

const patchXhr = (ctor: PatchableXhrConstructor, state: InterceptState): void => {
  const proto = ctor.prototype;
  const textDescriptor = Object.getOwnPropertyDescriptor(proto, "responseText");
  const responseDescriptor = Object.getOwnPropertyDescriptor(proto, "response");
  // getter が取れないのは想定外の実行環境。傍受を諦めて素通しさせる。
  if (textDescriptor?.get === undefined || responseDescriptor?.get === undefined) return;
  const originalText = textDescriptor.get;
  const originalResponse = responseDescriptor.get;

  // 変換対象として開かれたインスタンスだけを覚える。ここに無いインスタンスは
  // getter が元の値をそのまま返すので、非対象は素通しになる。
  const pending = new WeakMap<PatchableXhr, XhrPending>();

  const textOf = (xhr: PatchableXhr): string => {
    const raw = originalText.call(xhr) as string;
    const entry = pending.get(xhr);
    // 完了前の responseText は途中までの断片なので JSON として扱えない。
    if (entry === undefined || xhr.readyState !== XHR_DONE) return raw;
    entry.transformed ??= safeTransformText(raw, entry.transform);
    return entry.transformed;
  };

  const responseOf = (xhr: PatchableXhr): unknown => {
    const original = (): unknown => originalResponse.call(xhr);
    const entry = pending.get(xhr);
    if (entry === undefined || xhr.readyState !== XHR_DONE) return original();
    switch (xhr.responseType) {
      case "":
      case "text":
        return textOf(xhr);
      // ネイティブの getter は読むたびに同じ木を返すので、ここは変換結果を覚えない代わりに
      // 同じ木へ変換を当て直す。鍵を消すだけの変換なら結果は変わらない（tests/chat-images の
      // 「変換の冪等性」で固定）。冪等でない変換を登録するなら、ここに記憶を足す必要がある。
      case "json":
        return safeTransform(original(), entry.transform);
      // arraybuffer / blob / document は JSON として扱えない。
      default:
        return original();
    }
  };

  const originalOpen = proto.open;
  proto.open = function (this: PatchableXhr, method, url, ...rest) {
    const transform = transformFor(state, typeof url === "string" ? url : url.href);
    // インスタンスは別の URL に開き直され得るので、非対象なら覚えていた変換を捨てる。
    if (transform === undefined) pending.delete(this);
    else pending.set(this, { transform });
    originalOpen.apply(this, [method, url, ...rest]);
  };

  Object.defineProperty(proto, "responseText", {
    ...textDescriptor,
    get(this: PatchableXhr) {
      return textOf(this);
    },
  });
  Object.defineProperty(proto, "response", {
    ...responseDescriptor,
    get(this: PatchableXhr) {
      return responseOf(this);
    },
  });
};

const registryOf = (state: InterceptState): TransformRegistry => ({
  register: (target, transform) => {
    state.transforms.set(target, transform);
  },
});

/**
 * 傍受層を組み込み、変換関数の登録口を返す。
 *
 * 二重に呼んでもパッチは 1 回しか当たらない。SPA 遷移では JS コンテキストが維持される
 * ので初回 1 回で足りるが、多重注入でも壊れないようにしてある。
 */
export const installIntercept = (
  target: InterceptTarget = globalThis,
): TransformRegistry => {
  // マーカーは傍受層の実装詳細なので、公開する型には現れない。
  const marked = target as MarkedTarget;
  const installed = marked[STATE_KEY];
  if (installed !== undefined) return registryOf(installed);

  const state: InterceptState = { transforms: new Map() };
  marked[STATE_KEY] = state;
  patchFetch(target, state);
  patchXhr(target.XMLHttpRequest, state);
  return registryOf(state);
};
