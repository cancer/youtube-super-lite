import { afterAll, beforeAll, describe, expect, test } from "bun:test";

import { TAB_ID_REQUEST } from "../src/shared/tab-id";
import { tabKey } from "../src/shared/tab-store";

import { flush } from "./support/flush";

/**
 * service worker の配線。
 *
 * 他のテストのように依存を引数で渡さず chrome ごと差し替えるのは、検査したいものが
 * 「起動しただけで配線が行なわれる」という副作用そのものだから。sw は実物の chrome を
 * 配線する場所（合成点）なので、そこを通さずに配線の有無は確かめられない。
 *
 * 読み込みは 1 度しか起きない（モジュールは使い回される）ので、起動を 1 回だけ行ない、
 * 捕まえた配線に対して個々の検査を当てる。
 */

type MessageListener = (
  message: unknown,
  sender: { tab?: { id?: number } },
  sendResponse: (response: unknown) => void,
) => boolean;

const behaviors: unknown[] = [];
const accessLevels: unknown[] = [];
const messageListeners: MessageListener[] = [];
const removedListeners: ((tabId: number) => void)[] = [];
const removedKeys: string[][] = [];

/** 公開範囲の設定はテストから終わらせる。終わる前に答えていないことを見るため。 */
let openSession: () => void = () => {};

const sessionStored: Record<string, unknown> = {
  [tabKey(7, "chatDisplay")]: { fontSizePx: 20 },
  [tabKey(7, "equalizer")]: { voiceGainDb: 3 },
  [tabKey(9, "chatDisplay")]: { fontSizePx: 12 },
};

const globals = globalThis as unknown as { chrome?: unknown };
const original = globals.chrome;

beforeAll(async () => {
  globals.chrome = {
    runtime: {
      id: "test",
      onInstalled: { addListener: () => {} },
      onMessage: {
        addListener: (listener: MessageListener) => messageListeners.push(listener),
      },
    },
    sidePanel: {
      setPanelBehavior: (behavior: unknown): Promise<void> => {
        behaviors.push(behavior);
        return Promise.resolve();
      },
    },
    storage: {
      local: { get: async () => ({}), set: async () => {} },
      session: {
        setAccessLevel: (level: unknown): Promise<void> => {
          accessLevels.push(level);
          return new Promise((resolve) => {
            openSession = resolve;
          });
        },
        get: async () => sessionStored,
        remove: async (keys: string[]) => {
          removedKeys.push(keys);
        },
      },
    },
    tabs: {
      onRemoved: {
        addListener: (listener: (tabId: number) => void) =>
          removedListeners.push(listener),
      },
    },
  };

  await import("../src/background/sw");
});

afterAll(() => {
  globals.chrome = original;
});

describe("サイドパネル", () => {
  test("ツールバーのアイコンのクリックでサイドパネルが開くよう設定する", () => {
    expect(behaviors).toEqual([{ openPanelOnActionClick: true }]);
  });
});

describe("タブ単位の設定", () => {
  /** content script は session を読む。既定の公開範囲では読めないので広げる。 */
  test("session を content script からも読めるようにする", () => {
    expect(accessLevels).toEqual([
      { accessLevel: "TRUSTED_AND_UNTRUSTED_CONTEXTS" },
    ]);
  });

  test("問い合わせてきた content script のタブ番号を答える", async () => {
    let answer: unknown = "まだ答えていない";
    const returned = messageListeners[0](
      { type: TAB_ID_REQUEST },
      { tab: { id: 7 } },
      (response) => {
        answer = response;
      },
    );

    // 公開範囲を広げ終える前に答えると、受け取った側がまだ読めない session を読みに行く。
    expect(answer).toBe("まだ答えていない");
    expect(returned).toBe(true);

    openSession();
    await flush();

    expect(answer).toBe(7);
  });

  test("タブを持たない送り主には番号を答えない", async () => {
    let answer: unknown = "まだ答えていない";
    messageListeners[0]({ type: TAB_ID_REQUEST }, {}, (response) => {
      answer = response;
    });
    await flush();

    expect(answer).toBeUndefined();
  });

  test("別のメッセージには応答しない", () => {
    let answered = false;
    const returned = messageListeners[0](
      { type: "youtube-super-lite/別の用事" },
      { tab: { id: 7 } },
      () => {
        answered = true;
      },
    );

    expect(returned).toBe(false);
    expect(answered).toBe(false);
  });

  /** タブ番号は使い回される。消さないと、後から同じ番号になったタブが他人の設定を引き継ぐ。 */
  test("閉じたタブぶんの設定だけを捨てる", async () => {
    removedListeners[0](7);
    await flush();

    expect(removedKeys).toEqual([
      [tabKey(7, "chatDisplay"), tabKey(7, "equalizer")],
    ]);
  });

  test("捨てるものが無ければ storage を触らない", async () => {
    const before = removedKeys.length;

    removedListeners[0](11);
    await flush();

    expect(removedKeys).toHaveLength(before);
  });
});
