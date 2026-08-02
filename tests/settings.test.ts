import { describe, expect, test } from "bun:test";

import {
  CHAT_FONT_SIZE_PX,
  CHAT_PANEL_WIDTH_RATIO,
  chatDisplaySection,
  clampToRange,
  readSection,
  repairSection,
  watchDeclutterSection,
  watchSection,
  writeSection,
  type SettingsStore,
  type StoredChange,
} from "../src/shared/settings";

/**
 * chrome.storage.local のフェイク。保存済みの生データを直接覗ける形にしてあるのは、
 * 「読み出しでクランプされている」ことと「保存値そのものは触っていない」ことを
 * 別々に検証するため。
 */
const fakeStore = (
  stored: Record<string, unknown> = {},
): { store: SettingsStore; stored: Record<string, unknown> } => {
  const listeners = new Set<(changes: Record<string, StoredChange>) => void>();
  const store: SettingsStore = {
    get: async (keys) =>
      Object.fromEntries(
        keys.filter((key) => key in stored).map((key) => [key, stored[key]]),
      ),
    set: async (items) => {
      Object.assign(stored, items);
      const changes = Object.fromEntries(
        Object.entries(items).map(([key, value]) => [key, { newValue: value }]),
      );
      for (const listener of listeners) listener(changes);
    },
    onChanged: {
      addListener: (listener) => {
        listeners.add(listener);
      },
      removeListener: (listener) => {
        listeners.delete(listener);
      },
    },
  };
  return { store, stored };
};

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

  test("保存値が設定として壊れていても既定値で読み出せる", async () => {
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

  test("保存値が設定として壊れていても既定値で読み出せる", async () => {
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
