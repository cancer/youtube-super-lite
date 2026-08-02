import { afterEach, beforeEach, describe, expect, test } from "bun:test";

import {
  CHAT_FONT_SIZE_PX,
  CHAT_PANEL_WIDTH_RATIO,
  applySection,
  chatDisplaySection,
  clampToRange,
  readSection,
  repairSection,
  watchDeclutterSection,
  watchSection,
  writeSection,
  type SettingsStore,
} from "../src/shared/settings";

import { fakeStore } from "./support/settings-store";

describe("clampToRange", () => {
  test("下限未満の値を下限へ収める", () => {
    expect(clampToRange(CHAT_FONT_SIZE_PX, 9)).toBe(10);
  });

  test("上限超過の値を上限へ収める", () => {
    expect(clampToRange(CHAT_FONT_SIZE_PX, 29)).toBe(28);
  });

  test("比率の下限未満の値を下限へ収める", () => {
    expect(clampToRange(CHAT_PANEL_WIDTH_RATIO, 0.1)).toBe(0.15);
  });

  test("比率の上限超過の値を上限へ収める", () => {
    expect(clampToRange(CHAT_PANEL_WIDTH_RATIO, 0.9)).toBe(0.6);
  });

  test("範囲内の値はそのまま通す", () => {
    expect(clampToRange(CHAT_FONT_SIZE_PX, 20)).toBe(20);
  });

  test("下限ちょうどの値はそのまま通す", () => {
    expect(clampToRange(CHAT_FONT_SIZE_PX, 10)).toBe(10);
  });

  test("上限ちょうどの値はそのまま通す", () => {
    expect(clampToRange(CHAT_PANEL_WIDTH_RATIO, 0.6)).toBe(0.6);
  });

  test("数値でない値は既定値へ落とす", () => {
    expect(clampToRange(CHAT_FONT_SIZE_PX, "20")).toBe(16);
  });

  test("NaN は既定値へ落とす", () => {
    expect(clampToRange(CHAT_FONT_SIZE_PX, Number.NaN)).toBe(16);
  });

  test("未設定は既定値へ落とす", () => {
    expect(clampToRange(CHAT_PANEL_WIDTH_RATIO, undefined)).toBe(0.28);
  });
});

describe("readSection", () => {
  test("未保存なら既定の 16px / 0.28 を返す", async () => {
    const { store } = fakeStore();

    expect(await readSection(store, chatDisplaySection)).toEqual({
      fontSizePx: 16,
      panelWidthRatio: 0.28,
    });
  });

  test("保存済みの値を復元する", async () => {
    const { store } = fakeStore();
    await writeSection(store, chatDisplaySection, {
      fontSizePx: 22,
      panelWidthRatio: 0.4,
    });

    expect(await readSection(store, chatDisplaySection)).toEqual({
      fontSizePx: 22,
      panelWidthRatio: 0.4,
    });
  });

  test("範囲外の値が保存されていても読み出しは範囲内へ収める", async () => {
    const { store } = fakeStore({
      chatDisplay: { fontSizePx: 400, panelWidthRatio: 0.01 },
    });

    expect(await readSection(store, chatDisplaySection)).toEqual({
      fontSizePx: 28,
      panelWidthRatio: 0.15,
    });
  });

  test("チャット表示の保存値が壊れていても既定値で読み出せる", async () => {
    const { store } = fakeStore({ chatDisplay: "壊れた値" });

    expect(await readSection(store, chatDisplaySection)).toEqual({
      fontSizePx: 16,
      panelWidthRatio: 0.28,
    });
  });

  test("片方だけ保存されていても他方は既定値で埋まる", async () => {
    const { store } = fakeStore({ chatDisplay: { fontSizePx: 12 } });

    expect(await readSection(store, chatDisplaySection)).toEqual({
      fontSizePx: 12,
      panelWidthRatio: 0.28,
    });
  });
});

describe("readSection（watch ページの整理）", () => {
  test("未保存ならコメント欄を消す設定で始まる", async () => {
    const { store } = fakeStore();

    expect(await readSection(store, watchDeclutterSection)).toEqual({
      removeComments: true,
    });
  });

  test("消さない設定を保存していれば、それを復元する", async () => {
    const { store } = fakeStore();
    await writeSection(store, watchDeclutterSection, { removeComments: false });

    expect(await readSection(store, watchDeclutterSection)).toEqual({
      removeComments: false,
    });
  });

  test("真偽でない値が保存されていても既定値へ落とす", async () => {
    const { store } = fakeStore({ watchDeclutter: { removeComments: "false" } });

    expect(await readSection(store, watchDeclutterSection)).toEqual({
      removeComments: true,
    });
  });

  test("watch ページの整理の保存値が壊れていても既定値で読み出せる", async () => {
    const { store } = fakeStore({ watchDeclutter: "壊れた値" });

    expect(await readSection(store, watchDeclutterSection)).toEqual({
      removeComments: true,
    });
  });
});

describe("watchSection", () => {
  test("区画が変わったら正規化済みの値を渡す", async () => {
    const { store } = fakeStore();
    const received: unknown[] = [];
    watchSection(store, chatDisplaySection, (value) => received.push(value));

    await writeSection(store, chatDisplaySection, {
      fontSizePx: 99,
      panelWidthRatio: 0.5,
    });

    expect(received).toEqual([{ fontSizePx: 28, panelWidthRatio: 0.5 }]);
  });

  test("無関係なキーの変更は無視する", async () => {
    const { store } = fakeStore();
    const received: unknown[] = [];
    watchSection(store, chatDisplaySection, (value) => received.push(value));

    await store.set({ somethingElse: 1 });

    expect(received).toEqual([]);
  });

  test("購読を解除したら通知が止まる", async () => {
    const { store } = fakeStore();
    const received: unknown[] = [];
    const stop = watchSection(store, chatDisplaySection, (value) =>
      received.push(value),
    );

    stop();
    await writeSection(store, chatDisplaySection, {
      fontSizePx: 20,
      panelWidthRatio: 0.3,
    });

    expect(received).toEqual([]);
  });
});

describe("applySection", () => {
  test("読めた値を当てる", async () => {
    const { store } = fakeStore({ chatDisplay: { fontSizePx: 22 } });
    const applied: unknown[] = [];

    await applySection(store, chatDisplaySection, (value) => applied.push(value));

    expect(applied).toEqual([{ fontSizePx: 22, panelWidthRatio: 0.28 }]);
  });
});

/**
 * 拡張コンテキストの失効。
 *
 * unpacked 拡張を再読み込みすると、開いたままのページに残った content script や
 * サイドパネルは storage を触れなくなる。ページを開き直すまで回復しないので、
 * 設定の反映を止めることが正しい振る舞いで、例外の伝播も再試行も誤りになる。
 */
describe("拡張コンテキストの失効", () => {
  const originalDebug = console.debug;
  let debugMessages: unknown[][] = [];

  beforeEach(() => {
    debugMessages = [];
    console.debug = (...args: unknown[]): void => {
      debugMessages.push(args);
    };
  });

  afterEach(() => {
    console.debug = originalDebug;
  });

  test("失効した後の読み出しは例外を投げず、設定が無いことを返す", async () => {
    const { store, invalidate } = fakeStore({ chatDisplay: { fontSizePx: 22 } });
    invalidate();

    expect(await readSection(store, chatDisplaySection)).toBeUndefined();
  });

  test("読み出しの最中に失効しても例外は伝播しない", async () => {
    const { store, invalidate } = fakeStore();
    const racing: SettingsStore = {
      ...store,
      get: (keys) => {
        invalidate();
        return store.get(keys);
      },
    };

    expect(await readSection(racing, chatDisplaySection)).toBeUndefined();
  });

  test("失効していれば設定を当てない", async () => {
    const { store, invalidate } = fakeStore({ chatDisplay: { fontSizePx: 22 } });
    const applied: unknown[] = [];
    invalidate();

    await applySection(store, chatDisplaySection, (value) => applied.push(value));

    expect(applied).toEqual([]);
  });

  test("失効した後の書き込みは例外を投げず、保存もしない", async () => {
    const { store, stored, invalidate } = fakeStore();
    invalidate();

    await writeSection(store, chatDisplaySection, {
      fontSizePx: 20,
      panelWidthRatio: 0.3,
    });

    expect(stored.chatDisplay).toBeUndefined();
  });

  test("書き込みの最中に失効しても例外は伝播しない", async () => {
    const { store, invalidate } = fakeStore();
    const racing: SettingsStore = {
      ...store,
      set: (items) => {
        invalidate();
        return store.set(items);
      },
    };

    await writeSection(racing, chatDisplaySection, {
      fontSizePx: 20,
      panelWidthRatio: 0.3,
    });
  });

  test("購読の後に失効したら、以降の通知が止まる", async () => {
    const { store, invalidate } = fakeStore();
    const received: unknown[] = [];
    watchSection(store, chatDisplaySection, (value) => received.push(value));

    invalidate();
    await writeSection(store, chatDisplaySection, {
      fontSizePx: 20,
      panelWidthRatio: 0.3,
    });

    expect(received).toEqual([]);
  });

  test("失効した後に購読を始めても例外を投げない", () => {
    const { store, invalidate } = fakeStore();
    invalidate();

    expect(() => watchSection(store, chatDisplaySection, () => {})).not.toThrow();
  });

  test("失効した後に購読を解除しても例外を投げない", () => {
    const { store, invalidate } = fakeStore();
    const stop = watchSection(store, chatDisplaySection, () => {});

    invalidate();

    expect(stop).not.toThrow();
  });

  test("失効の報告は何度触っても 1 度だけ", async () => {
    const { store, invalidate } = fakeStore();
    invalidate();

    await readSection(store, chatDisplaySection);
    await readSection(store, chatDisplaySection);
    await writeSection(store, chatDisplaySection, chatDisplaySection.defaults);
    watchSection(store, chatDisplaySection, () => {})();

    expect(debugMessages.length).toBe(1);
  });

  test("失効していない読み出しの失敗は呼び出し側へ伝わる", async () => {
    const { store } = fakeStore();
    const broken: SettingsStore = {
      ...store,
      get: async () => {
        throw new Error("storage が壊れた");
      },
    };

    await expect(readSection(broken, chatDisplaySection)).rejects.toThrow(
      "storage が壊れた",
    );
  });

  test("失効していない書き込みの失敗は呼び出し側へ伝わる", async () => {
    const { store } = fakeStore();
    const broken: SettingsStore = {
      ...store,
      set: async () => {
        throw new Error("storage が壊れた");
      },
    };

    await expect(
      writeSection(broken, chatDisplaySection, chatDisplaySection.defaults),
    ).rejects.toThrow("storage が壊れた");
  });
});

describe("repairSection", () => {
  test("範囲外の保存値を範囲内へ書き戻す", async () => {
    const { store, stored } = fakeStore({
      chatDisplay: { fontSizePx: 400, panelWidthRatio: 0.9 },
    });

    await repairSection(store, chatDisplaySection);

    expect(stored.chatDisplay).toEqual({
      fontSizePx: 28,
      panelWidthRatio: 0.6,
    });
  });

  test("未保存なら既定値を保存する", async () => {
    const { store, stored } = fakeStore();

    await repairSection(store, chatDisplaySection);

    expect(stored.chatDisplay).toEqual({
      fontSizePx: 16,
      panelWidthRatio: 0.28,
    });
  });
});
