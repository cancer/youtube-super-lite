/**
 * content script を注入された YouTube 上の面。
 *
 * どちらの world にも同じ content script が入り、どちらも「今どの面か」を必要とするので、
 * 面の知識は world 固有ではない。判定はこのモジュールだけが持ち、pathname を直接見る箇所を
 * 各 world に増やさない。
 */
export type Surface = "watch" | "live_chat" | "other";

/**
 * pathname から面を判定する。
 *
 * 判定は manifest の `matches`（`/watch*` と `/live_chat*`）と対応させてある。宣言である
 * manifest は統合できないので、対応がズレたら落ちるテストを tests/surface.test.ts に置いてある。
 * リプレイのチャット（`/live_chat_replay`）は `/live_chat*` に含まれるため live_chat として扱う。
 */
export const surfaceOf = (pathname: string): Surface => {
  if (pathname.startsWith("/watch")) return "watch";
  if (pathname.startsWith("/live_chat")) return "live_chat";
  return "other";
};
