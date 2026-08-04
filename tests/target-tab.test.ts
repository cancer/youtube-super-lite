import { describe, expect, test } from "bun:test";

import {
  chatDisplaySection,
  type ChatDisplaySettings,
  type SettingsStore,
} from "../src/shared/settings";
import { tabKey, tabScopedStore } from "../src/shared/tab-store";
import { createTargetTab, type ActiveTab } from "../src/side-panel/target-tab";

import { flush } from "./support/flush";
import { fakeStore } from "./support/settings-store";

/**
 * サイドパネルが相手にするタブ。
 *
 * ここで固定するのは「操作の相手は今見ているタブ 1 つに限る」こと。他に開いているタブが
 * 巻き添えで変わらないことと、タブを切り替えたらパネルの表示がそのタブのものになることを、
 * 同じ 1 つの決まりごとの表裏として押さえる。
 */

const display = (fontSizePx: number): ChatDisplaySettings => ({
  fontSizePx,
  panelWidthRatio: chatDisplaySection.defaults.panelWidthRatio,
});

/** タブと窓の組。既定はどれも同じ窓（窓をまたぐ切り替えを見るときだけ変える）。 */
const at = (tabId: number, windowId = 1): ActiveTab => ({ tabId, windowId });

const setup = ({
  persisted = display(16),
  tabs = {} as Record<number, ChatDisplaySettings>,
  active = at(1) as ActiveTab | undefined,
  storeOfTab,
}: {
  persisted?: ChatDisplaySettings;
  tabs?: Record<number, ChatDisplaySettings>;
  active?: ActiveTab | undefined;
  storeOfTab?: (session: SettingsStore) => (tabId: number) => SettingsStore;
} = {}) => {
  const persistent = fakeStore({ [chatDisplaySection.key]: persisted });
  const session = fakeStore(
    Object.fromEntries(
      Object.entries(tabs).map(([tabId, value]) => [
        tabKey(Number(tabId), chatDisplaySection.key),
        value,
      ]),
    ),
  );
  const activations: ((tab: ActiveTab) => void)[] = [];
  const rendered: number[] = [];

  const target = createTargetTab({
    activeTab: async () => active,
    onActivated: (listener) => activations.push(listener),
    storeOfTab: storeOfTab
      ? storeOfTab(session.store)
      : (tabId) => tabScopedStore(session.store, tabId),
    persistent: persistent.store,
    sections: [chatDisplaySection],
  });

  target.follow(chatDisplaySection, (value) => {
    rendered.push(value.fontSizePx);
  });

  return {
    target,
    persistent,
    session,
    rendered,
    activate: (tab: ActiveTab) => {
      for (const listener of activations) listener(tab);
    },
  };
};

describe("相手のタブを決める", () => {
  test("見ているタブの値を描く", async () => {
    const { target, rendered } = setup({ tabs: { 1: display(24) } });

    await target.start();
    await flush();

    expect(rendered).toEqual([24]);
  });

  test("まだ何も設定していないタブには、永続の保存値が入る", async () => {
    const { target, session, rendered } = setup({ persisted: display(22) });

    await target.start();
    await flush();

    expect(session.stored[tabKey(1, chatDisplaySection.key)]).toEqual(display(22));
    expect(rendered).toEqual([22]);
  });

  test("タブを切り替えると、そのタブの値を描く", async () => {
    const { target, rendered, activate } = setup({
      tabs: { 1: display(24), 2: display(12) },
    });
    await target.start();
    await flush();

    activate(at(2));
    await flush();

    expect(rendered).toEqual([24, 12]);
  });

  test("他の窓でのタブの切り替えには反応しない", async () => {
    const { target, rendered, activate } = setup({
      tabs: { 1: display(24), 2: display(12) },
    });
    await target.start();
    await flush();

    activate(at(2, 2));
    await flush();

    expect(rendered).toEqual([24]);
  });

  test("相手のタブが分からなければ、永続の保存値を描く", async () => {
    const { target, rendered } = setup({ active: undefined, persisted: display(18) });

    await target.start();
    await flush();

    expect(rendered).toEqual([18]);
  });
});

describe("相手のタブへ当てる", () => {
  test("見ているタブへ当たる", async () => {
    const { target, session } = setup();
    await target.start();

    await target.apply(chatDisplaySection, display(26));

    expect(session.stored[tabKey(1, chatDisplaySection.key)]).toEqual(display(26));
  });

  /** 直したかった不具合そのもの。開いている他のタブは巻き添えで変わらない。 */
  test("他のタブの値は変わらない", async () => {
    const { target, session } = setup({ tabs: { 2: display(16) } });
    await target.start();

    await target.apply(chatDisplaySection, display(26));

    expect(session.stored[tabKey(2, chatDisplaySection.key)]).toEqual(display(16));
  });

  // 永続の保存値は「次に開くタブが最初に使う値」。R4 / R5 の永続化はここで満たす。
  test("次に開くタブの初期値としても残す", async () => {
    const { target, persistent } = setup();
    await target.start();

    await target.apply(chatDisplaySection, display(26));

    expect(persistent.stored[chatDisplaySection.key]).toEqual(display(26));
  });

  /**
   * フィールド単位の当て方。区画を複数の操作面で分担しているとき（R5 の幅はページ内のハンドル）、
   * パネル側の操作が相手のフィールドを巻き込まないこと。
   */
  test("フィールド単位で当てると、他のフィールドは残る", async () => {
    const { target, session, persistent } = setup();
    await target.start();

    await target.patch(chatDisplaySection, { fontSizePx: 26 });

    expect(session.stored[tabKey(1, chatDisplaySection.key)]).toEqual({
      ...display(26),
    });
    expect(persistent.stored[chatDisplaySection.key]).toEqual({ ...display(26) });
  });

  test("相手のタブで値が変わればパネルも描き直す", async () => {
    const { target, session, rendered } = setup();
    await target.start();
    await flush();

    await session.store.set({ [tabKey(1, chatDisplaySection.key)]: display(28) });

    expect(rendered.at(-1)).toBe(28);
  });

  test("切り替えた後は、前のタブの値が変わってもパネルは描き直さない", async () => {
    const { target, session, rendered, activate } = setup({
      tabs: { 1: display(24), 2: display(12) },
    });
    await target.start();
    await flush();
    activate(at(2));
    await flush();

    await session.store.set({ [tabKey(1, chatDisplaySection.key)]: display(28) });

    expect(rendered.at(-1)).toBe(12);
  });
});

/**
 * 読み出しは非同期なので、切り替えが続くと前の相手の値が後から届き得る。届いてから捨てる
 * ことでしか防げない（読み出しを取り消す手段が無い）ので、捨てていることを直接押さえる。
 */
describe("追い越された読み出し", () => {
  /** 1 度目の読み出し（初期値を入れるための確認）だけ即答し、以後を遅らせる保存先。 */
  const slowAfterFirstRead = (store: SettingsStore): SettingsStore => {
    let reads = 0;
    return {
      ...store,
      get: async (keys) => {
        reads += 1;
        if (reads > 1) await flush();
        return store.get(keys);
      },
    };
  };

  test("前の相手の値が後から届いても描かない", async () => {
    const { target, rendered, activate } = setup({
      tabs: { 1: display(24), 2: display(12) },
      storeOfTab: (session) => (tabId) =>
        tabId === 1
          ? slowAfterFirstRead(tabScopedStore(session, tabId))
          : tabScopedStore(session, tabId),
    });

    // 1 つ目のタブの読み出しが宙に浮いた状態にしてから切り替える。
    void target.start();
    await flush();
    activate(at(2));
    await flush();
    await flush();

    expect(rendered).toEqual([12]);
  });
});
