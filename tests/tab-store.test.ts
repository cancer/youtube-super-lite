import { describe, expect, test } from "bun:test";

import {
  chatDisplaySection,
  readSection,
  watchSection,
  writeSection,
} from "../src/shared/settings";
import { keysOfTab, tabKey, tabScopedStore } from "../src/shared/tab-store";

import { fakeStore } from "./support/settings-store";

/**
 * 設定をタブごとに分ける保存先。
 *
 * ここで固定するのは 2 つ。「隣のタブの値と混ざらないこと」と「渡された側がタブ単位であることを
 * 知らずに済むこと」（読み書きも変更通知も区画のキーのまま扱える）。
 */

describe("tabScopedStore", () => {
  test("書いた値は、そのタブのキーの下に入る", async () => {
    const { store, stored } = fakeStore();

    await writeSection(
      tabScopedStore(store, 7),
      chatDisplaySection,
      chatDisplaySection.defaults,
    );

    expect(stored).toEqual({
      [tabKey(7, chatDisplaySection.key)]: chatDisplaySection.defaults,
    });
  });

  test("読み出しは自分のタブの値を返す", async () => {
    const { store } = fakeStore({
      [tabKey(7, chatDisplaySection.key)]: { fontSizePx: 20, panelWidthRatio: 0.4 },
      [tabKey(8, chatDisplaySection.key)]: { fontSizePx: 12, panelWidthRatio: 0.2 },
    });

    const value = await readSection(tabScopedStore(store, 7), chatDisplaySection);

    expect(value).toEqual({ fontSizePx: 20, panelWidthRatio: 0.4 });
  });

  test("他のタブの値は読めない（自分のタブに保存が無ければ既定値になる）", async () => {
    const { store } = fakeStore({
      [tabKey(8, chatDisplaySection.key)]: { fontSizePx: 12, panelWidthRatio: 0.2 },
    });

    const value = await readSection(tabScopedStore(store, 7), chatDisplaySection);

    expect(value).toEqual(chatDisplaySection.defaults);
  });

  test("自分のタブの変更は区画のキーのまま届く", async () => {
    const { store } = fakeStore();
    const seen: unknown[] = [];
    watchSection(tabScopedStore(store, 7), chatDisplaySection, (value) => {
      seen.push(value);
    });

    await writeSection(
      tabScopedStore(store, 7),
      chatDisplaySection,
      chatDisplaySection.defaults,
    );

    expect(seen).toEqual([chatDisplaySection.defaults]);
  });

  test("他のタブの変更では呼ばれない", async () => {
    const { store } = fakeStore();
    const seen: unknown[] = [];
    watchSection(tabScopedStore(store, 7), chatDisplaySection, (value) => {
      seen.push(value);
    });

    await writeSection(
      tabScopedStore(store, 8),
      chatDisplaySection,
      chatDisplaySection.defaults,
    );

    expect(seen).toEqual([]);
  });

  test("購読を解除すると届かなくなる", async () => {
    const { store } = fakeStore();
    const seen: unknown[] = [];
    const scoped = tabScopedStore(store, 7);
    const unsubscribe = watchSection(scoped, chatDisplaySection, (value) => {
      seen.push(value);
    });

    unsubscribe();
    await writeSection(scoped, chatDisplaySection, chatDisplaySection.defaults);

    expect(seen).toEqual([]);
  });
});

describe("keysOfTab", () => {
  test("そのタブのキーだけを選ぶ", () => {
    const keys = [
      tabKey(4, "chatDisplay"),
      tabKey(4, "equalizer"),
      tabKey(42, "chatDisplay"),
      "chatDisplay",
    ];

    expect(keysOfTab(keys, 4)).toEqual([
      tabKey(4, "chatDisplay"),
      tabKey(4, "equalizer"),
    ]);
  });

  /** 番号は前方一致で見るので、tab4 が tab42 を巻き込まないことを別に押さえる。 */
  test("番号の先頭が同じ別のタブを巻き込まない", () => {
    expect(keysOfTab([tabKey(42, "chatDisplay")], 4)).toEqual([]);
  });
});
