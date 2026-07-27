/** content script を注入された YouTube 上の面。 */
export type Surface = "watch" | "live_chat" | "other";

/**
 * pathname から面を判定する。
 *
 * 判定は manifest の `matches`（`/watch*` と `/live_chat*`）と対応させてある。
 * リプレイのチャット（`/live_chat_replay`）は `/live_chat*` に含まれるため live_chat として扱う。
 */
export const surfaceOf = (pathname: string): Surface => {
  if (pathname.startsWith("/watch")) return "watch";
  if (pathname.startsWith("/live_chat")) return "live_chat";
  return "other";
};
