/**
 * YouTube の SPA 遷移（要件 R7）。
 *
 * 遷移でドキュメントは再ロードされないため document_start の content script は初回しか走らない。
 * 標準 API に当てるパッチ（R2 / R6）は JS コンテキストが維持されるので 1 回で足りるが、
 * DOM を対象にする処理（R3 / R5）は遷移ごとに当て直す必要がある。
 */

/** 遷移完了イベント。YouTube の SPA ルータが発火させる。 */
const NAVIGATE_FINISH = "yt-navigate-finish";

const DOM_READY = "DOMContentLoaded";

/**
 * 遷移イベントの発火元。実体は document で、テストではフェイクを渡す。
 *
 * 必要な操作だけを並べてあるので、readyState を持たない EventTarget では代用できないこと
 * （初回適用のために構築状態を見る）が型から分かる。
 */
export type NavigationSource = {
  readonly readyState: DocumentReadyState;
  addEventListener(
    type: string,
    listener: () => void,
    options?: { once?: boolean },
  ): void;
  removeEventListener(type: string, listener: () => void): void;
};

/**
 * SPA 遷移ごとに apply を呼ぶ。戻り値を呼ぶと購読を解除する。
 *
 * 初回は DOM が使える時点でも 1 回呼ぶ。cold load で yt-navigate-finish が飛ぶ保証がないうえ、
 * 初回適用と遷移後の再適用を同じ経路に載せておけば、呼び出し側は「今が初回か」を判定しなくてよい。
 * apply は同じ状態に何度当てても壊れない（冪等な）実装であることを前提にする。
 */
export const onNavigated = (
  apply: () => void,
  source: NavigationSource = document,
): (() => void) => {
  const handler = (): void => apply();
  source.addEventListener(NAVIGATE_FINISH, handler);
  if (source.readyState === "loading") {
    source.addEventListener(DOM_READY, handler, { once: true });
  } else {
    handler();
  }
  return () => {
    source.removeEventListener(NAVIGATE_FINISH, handler);
    source.removeEventListener(DOM_READY, handler);
  };
};
