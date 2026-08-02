import { describe, expect, test } from "bun:test";

import { onNavigated, type NavigationSource } from "../src/shared/navigation";

/** document のフェイク。readyState を固定して、初回適用の分岐を両方たどれるようにする。 */
const fakeSource = (
  readyState: DocumentReadyState,
): { source: NavigationSource; dispatch: (type: string) => void } => {
  const target = new EventTarget();
  return {
    source: {
      readyState,
      addEventListener: (type, listener, options) =>
        target.addEventListener(type, listener, options),
      removeEventListener: (type, listener) =>
        target.removeEventListener(type, listener),
    },
    dispatch: (type) => {
      target.dispatchEvent(new Event(type));
    },
  };
};

describe("onNavigated", () => {
  test("DOM が使える状態なら登録した時点で 1 回適用する", () => {
    const { source } = fakeSource("complete");
    let applied = 0;

    onNavigated(() => applied++, source);

    expect(applied).toBe(1);
  });

  test("DOM 構築中なら DOMContentLoaded まで適用を待つ", () => {
    const { source } = fakeSource("loading");
    let applied = 0;

    onNavigated(() => applied++, source);

    expect(applied).toBe(0);
  });

  test("DOM 構築中に登録したら DOMContentLoaded で適用する", () => {
    const { source, dispatch } = fakeSource("loading");
    let applied = 0;
    onNavigated(() => applied++, source);

    dispatch("DOMContentLoaded");

    expect(applied).toBe(1);
  });

  test("SPA 遷移の完了ごとに適用する", () => {
    const { source, dispatch } = fakeSource("complete");
    let applied = 0;
    onNavigated(() => applied++, source);

    dispatch("yt-navigate-finish");
    dispatch("yt-navigate-finish");

    expect(applied).toBe(3);
  });

  test("解除したら遷移で適用しない", () => {
    const { source, dispatch } = fakeSource("complete");
    let applied = 0;
    const stop = onNavigated(() => applied++, source);

    stop();
    dispatch("yt-navigate-finish");

    expect(applied).toBe(1);
  });

  test("解除したら DOMContentLoaded を待っていた初回適用も起きない", () => {
    const { source, dispatch } = fakeSource("loading");
    let applied = 0;
    const stop = onNavigated(() => applied++, source);

    stop();
    dispatch("DOMContentLoaded");

    expect(applied).toBe(0);
  });
});
