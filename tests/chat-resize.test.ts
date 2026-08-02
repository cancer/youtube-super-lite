import { describe, expect, test } from "bun:test";

import {
  CHAT_RESIZE_HANDLE_ID,
  CHAT_RESIZE_HOST_SELECTOR,
  CHAT_RESIZE_RETRY_INTERVAL_MS,
  draggedRatio,
  startChatResize,
  type ChatResizeOptions,
  type DragPointer,
  type Handle,
  type HandleHost,
} from "../src/isolated/chat-resize";
import { chatDisplaySection } from "../src/shared/settings";

import { flush } from "./support/flush";
import { fakeStore } from "./support/settings-store";

/**
 * ページ内のハンドルは実ブラウザでしか触れないので、ここで固定するのは「掴んだぶんが
 * どの値になるか」と「いつ保存するか」だけ。実 DOM に当たるかどうか（差し込み先のセレクタ・
 * 重ね方）は実ブラウザで確認した文字列として押さえる。
 */

/** つまみのフェイク。listener と style を覗ける形にしてある。 */
const fakeHandle = (): {
  handle: Handle;
  attributes: Map<string, string>;
  styles: Map<string, string>;
  captured: number[];
  fire: (type: string, event: DragPointer) => void;
} => {
  const listeners = new Map<string, ((event: DragPointer) => void)[]>();
  const attributes = new Map<string, string>();
  const styles = new Map<string, string>();
  const captured: number[] = [];
  const handle: Handle = {
    id: "",
    title: "",
    style: {
      setProperty: (property, value) => {
        styles.set(property, value);
      },
    },
    setAttribute: (name, value) => {
      attributes.set(name, value);
    },
    addEventListener: (type, listener) => {
      listeners.set(type, [...(listeners.get(type) ?? []), listener]);
    },
    setPointerCapture: (pointerId) => {
      captured.push(pointerId);
    },
  };
  return {
    handle,
    attributes,
    styles,
    captured,
    fire: (type, event) => {
      for (const listener of listeners.get(type) ?? []) listener(event);
    },
  };
};

/** 差し込み先のフェイク。差し込まれたつまみと、指定した位置の基準を覗ける。 */
const fakeHost = (
  clientWidth: number,
): { host: HandleHost; inserted: { id: string }[]; styles: Map<string, string> } => {
  const inserted: { id: string }[] = [];
  const styles = new Map<string, string>();
  return {
    host: {
      clientWidth,
      style: {
        setProperty: (property, value) => {
          styles.set(property, value);
        },
      },
      querySelector: (selectors) =>
        inserted.find((element) => selectors === `#${element.id}`) ?? null,
      insertAdjacentElement: (_position, element) => {
        inserted.push(element);
        return element;
      },
    },
    inserted,
    styles,
  };
};

const pointer = (clientX: number, pointerId = 1): DragPointer & {
  prevented: () => boolean;
} => {
  let prevented = false;
  return {
    clientX,
    pointerId,
    preventDefault: () => {
      prevented = true;
    },
    prevented: () => prevented,
  };
};

/** 幅を当てる先（documentElement）のフェイク。 */
const fakeStyleHost = (): {
  styleHost: ChatResizeOptions["styleHost"];
  variables: Map<string, string>;
} => {
  const variables = new Map<string, string>();
  return {
    styleHost: {
      documentElement: {
        style: {
          setProperty: (property, value) => {
            variables.set(property, value);
          },
        },
        insertAdjacentElement: () => undefined,
      },
      createElement: () => ({ id: "", textContent: null }),
      getElementById: () => null,
    },
    variables,
  };
};

const start = (
  options: {
    readonly stored?: Record<string, unknown>;
    readonly clientWidth?: number;
    readonly viewportWidth?: number;
    readonly hostMissing?: boolean;
  } = {},
) => {
  const { store, stored } = fakeStore(options.stored ?? {});
  const persistentStore = fakeStore();
  const handle = fakeHandle();
  const host = fakeHost(options.clientWidth ?? 400);
  const style = fakeStyleHost();
  const scheduled: (() => void)[] = [];
  startChatResize({
    store,
    persistent: persistentStore.store,
    findHost: () => (options.hostMissing === true ? null : host.host),
    createHandle: () => handle.handle,
    styleHost: style.styleHost,
    viewportWidth: () => options.viewportWidth ?? 1000,
    schedule: (task) => {
      scheduled.push(task);
    },
  });
  return {
    handle,
    host,
    stored,
    persisted: persistentStore.stored,
    variables: style.variables,
    tick: () => {
      for (const task of scheduled) task();
    },
    panelWidth: () => style.variables.get("--youtube-super-lite-chat-panel-width"),
    savedRatio: (saved: Record<string, unknown>): unknown =>
      (saved.chatDisplay as { panelWidthRatio?: unknown } | undefined)
        ?.panelWidthRatio,
  };
};

describe("draggedRatio", () => {
  // 列は右端が固定で左端が動くので、左（clientX が小さい側）へ動かすほど広くなる。
  test("左へ動かすと広くなる", () => {
    expect(draggedRatio({ clientX: 700, widthPx: 300 }, 600, 1000)).toBe(0.4);
  });

  test("右へ動かすと狭くなる", () => {
    expect(draggedRatio({ clientX: 700, widthPx: 300 }, 800, 1000)).toBe(0.2);
  });

  test("動かさなければ掴んだ時点の幅のまま", () => {
    expect(draggedRatio({ clientX: 700, widthPx: 300 }, 700, 1000)).toBe(0.3);
  });

  test("上限を超える幅は上限で止まる", () => {
    expect(draggedRatio({ clientX: 700, widthPx: 300 }, 0, 1000)).toBe(0.6);
  });

  test("下限に満たない幅は下限で止まる", () => {
    expect(draggedRatio({ clientX: 700, widthPx: 300 }, 1000, 1000)).toBe(0.15);
  });

  // 幅が数値にならない状況（画面幅 0）でも、当てる値は必ず範囲内であること。
  test("画面幅が 0 なら既定値になる", () => {
    expect(draggedRatio({ clientX: 700, widthPx: 300 }, 600, 0)).toBe(
      chatDisplaySection.defaults.panelWidthRatio,
    );
  });
});

describe("ハンドルの差し込み", () => {
  test("差し込み先へ入れる", () => {
    const { handle, host } = start();

    expect(host.inserted).toEqual([handle.handle]);
  });

  test("id と説明を持つ", () => {
    const { handle } = start();

    expect(handle.handle.id).toBe(CHAT_RESIZE_HANDLE_ID);
    expect(handle.handle.title).not.toBe("");
  });

  /** 掴める場所が見えなければハンドルは無いのと同じ。太さ・カーソル・重なり順を固定する。 */
  test("列の左端に重なる見た目を持つ", () => {
    const { handle } = start();

    expect(handle.styles.get("position")).toBe("absolute");
    expect(handle.styles.get("left")).toBe("0");
    expect(handle.styles.get("cursor")).toBe("col-resize");
    expect(Number(handle.styles.get("width")?.replace("px", ""))).toBeGreaterThan(0);
    // チャットの枠が持つ z-index（実測 600）より前に出ていないと掴めない。
    expect(Number(handle.styles.get("z-index"))).toBeGreaterThan(600);
  });

  test("重ねる位置の基準を差し込み先に持たせる", () => {
    const { host } = start();

    expect(host.styles.get("position")).toBe("relative");
  });

  // 差し込み先は遷移で作り直される。周期で引き直しても二重に入れないこと。
  test("引き直しても二重に入れない", () => {
    const { host, tick } = start();

    tick();
    tick();

    expect(host.inserted).toHaveLength(1);
  });

  // 外されたら入れ直す（YouTube 側の作り直しでハンドルごと消える）。
  test("外されていたら入れ直す", () => {
    const { host, handle, tick } = start();
    host.inserted.length = 0;

    tick();

    expect(host.inserted).toEqual([handle.handle]);
  });

  test("差し込み先が無ければ何もしない", () => {
    const { host, tick } = start({ hostMissing: true });

    tick();

    expect(host.inserted).toEqual([]);
  });
});

describe("ドラッグ", () => {
  test("掴んで動かすと幅がその場で変わる", () => {
    const { handle, panelWidth } = start({ clientWidth: 300 });

    handle.fire("pointerdown", pointer(700));
    handle.fire("pointermove", pointer(600));

    expect(panelWidth()).toBe("calc(0.4 * 100vw)");
  });

  test("掴んでいる間はポインタを取り込む", () => {
    const { handle } = start();

    handle.fire("pointerdown", pointer(700, 42));

    expect(handle.captured).toEqual([42]);
  });

  // 掴んだところからページ側の文字選択が始まると、ドラッグ中の見た目が荒れる。
  test("掴んだときに既定の動作を止める", () => {
    const { handle } = start();
    const event = pointer(700);

    handle.fire("pointerdown", event);

    expect(event.prevented()).toBe(true);
  });

  test("掴んでいなければ動かしても何も起きない", () => {
    const { handle, panelWidth } = start();

    handle.fire("pointermove", pointer(600));

    expect(panelWidth()).toBeUndefined();
  });

  test("離した後の動きには反応しない", () => {
    const { handle, panelWidth } = start({ clientWidth: 300 });
    handle.fire("pointerdown", pointer(700));
    handle.fire("pointermove", pointer(600));
    handle.fire("pointerup", pointer(600));

    handle.fire("pointermove", pointer(500));

    expect(panelWidth()).toBe("calc(0.4 * 100vw)");
  });
});

describe("ドラッグの保存", () => {
  test("離したときに保存する", async () => {
    const { handle, stored, savedRatio } = start({ clientWidth: 300 });

    handle.fire("pointerdown", pointer(700));
    handle.fire("pointermove", pointer(600));
    handle.fire("pointerup", pointer(600));
    await flush();

    expect(savedRatio(stored)).toBe(0.4);
  });

  // 次に開くタブが最初に使う値にもなる（サイドパネルでの操作と同じ扱い）。
  test("次に開くタブの初期値にも残す", async () => {
    const { handle, persisted, savedRatio } = start({ clientWidth: 300 });

    handle.fire("pointerdown", pointer(700));
    handle.fire("pointermove", pointer(600));
    handle.fire("pointerup", pointer(600));
    await flush();

    expect(savedRatio(persisted)).toBe(0.4);
  });

  // 区画ごと書き換わるので、幅だけを差し替えて他の設定を保つこと。
  test("同じ区画の文字サイズは保つ", async () => {
    const { handle, stored } = start({
      clientWidth: 300,
      stored: { chatDisplay: { fontSizePx: 22, panelWidthRatio: 0.3 } },
    });

    handle.fire("pointerdown", pointer(700));
    handle.fire("pointermove", pointer(600));
    handle.fire("pointerup", pointer(600));
    await flush();

    expect(stored.chatDisplay).toEqual({ fontSizePx: 22, panelWidthRatio: 0.4 });
  });

  // 動かすたびに保存すると幅が手から遅れて付いてくる。保存は離した 1 度だけ。
  test("動かしている間は保存しない", async () => {
    const { handle, stored } = start({ clientWidth: 300 });

    handle.fire("pointerdown", pointer(700));
    handle.fire("pointermove", pointer(650));
    handle.fire("pointermove", pointer(600));
    await flush();

    expect(stored.chatDisplay).toBeUndefined();
  });

  test("掴んだだけで動かしていなければ保存しない", async () => {
    const { handle, stored } = start();

    handle.fire("pointerdown", pointer(700));
    handle.fire("pointerup", pointer(700));
    await flush();

    expect(stored.chatDisplay).toBeUndefined();
  });

  /**
   * 中断（他のジェスチャに取られた等）でも、その時点の幅は既に当たっている。保存しないと
   * 「今は広いのに、開き直すと戻る」という食い違いが残る。
   */
  test("中断されたときも当たっている幅を保存する", async () => {
    const { handle, stored, savedRatio } = start({ clientWidth: 300 });

    handle.fire("pointerdown", pointer(700));
    handle.fire("pointermove", pointer(600));
    handle.fire("pointercancel", pointer(600));
    await flush();

    expect(savedRatio(stored)).toBe(0.4);
  });
});

/**
 * 差し込み先のセレクタ。
 *
 * 出所: 2026-08-02 に実ブラウザ（未ログイン）の配信中のライブの watch ページで確認した DOM。
 * 実 DOM に当たるかどうかは実ブラウザでしか確かめられないので、確認した文字列を固定して、
 * 書き換えるなら再確認が要ることを示す。
 */
describe("差し込み先", () => {
  test("watch ページのチャットの入れ物を名指しする", () => {
    expect(CHAT_RESIZE_HOST_SELECTOR).toBe("ytd-watch-flexy #chat-container");
  });

  test("引き直しの周期を持つ", () => {
    expect(CHAT_RESIZE_RETRY_INTERVAL_MS).toBeGreaterThan(0);
  });
});
