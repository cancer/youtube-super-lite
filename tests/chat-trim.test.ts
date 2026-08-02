import { describe, expect, test } from "bun:test";

import {
  CHAT_ITEM_LIMIT,
  CHAT_TRIM_INTERVAL_MS,
  startChatTrim,
  trimChatItems,
  type ChatItemList,
} from "../src/isolated/chat-trim";

/**
 * チャット項目の DOM 上限（R3 の DOM 層）。
 *
 * 実要素を用意せず、上限判定に要る 3 つの操作（件数・先頭・除去）だけを持つフェイクで検査する。
 */
const fakeList = (count: number): ChatItemList & { readonly removed: number } => {
  let items = count;
  let removed = 0;
  return {
    get childElementCount() {
      return items;
    },
    get firstElementChild() {
      // 実要素と同じく、子が無ければ先頭も無い。
      if (items === 0) return null;
      return {
        remove: () => {
          items -= 1;
          removed += 1;
        },
      };
    },
    get removed() {
      return removed;
    },
  };
};

describe("trimChatItems", () => {
  test("上限以下なら何も外さない", () => {
    const list = fakeList(3);

    trimChatItems(list, 3);

    expect(list.removed).toBe(0);
  });

  test("超過分だけ外す", () => {
    const list = fakeList(10);

    trimChatItems(list, 4);

    expect(list.removed).toBe(6);
  });

  // 古い発言から外す。YouTube のチャットは古い順に並び、新着が末尾に付く。
  test("外すのは先頭（＝古い側）だけで、上限ちょうどまで残す", () => {
    const list = fakeList(10);

    trimChatItems(list, 4);

    expect(list.childElementCount).toBe(4);
  });

  test("空でも何も起きない", () => {
    const list = fakeList(0);

    trimChatItems(list, 4);

    expect(list.removed).toBe(0);
  });

  // 適用先は querySelector の結果なので、まだ描画されていない間は null で来る。
  test("適用先が未生成（null）なら何もしない", () => {
    expect(() => trimChatItems(null, CHAT_ITEM_LIMIT)).not.toThrow();
  });
});

/** 起動時に渡された処理と間隔を記録するだけのスケジューラ。 */
const fakeScheduler = (): {
  schedule: (task: () => void, intervalMs: number) => void;
  run: () => void;
  readonly intervalMs: number | undefined;
  readonly runCount: number;
} => {
  let task: (() => void) | undefined;
  let intervalMs: number | undefined;
  let runCount = 0;
  return {
    schedule: (scheduled, ms) => {
      task = scheduled;
      intervalMs = ms;
    },
    run: () => {
      runCount += 1;
      task?.();
    },
    get intervalMs() {
      return intervalMs;
    },
    get runCount() {
      return runCount;
    },
  };
};

describe("startChatTrim", () => {
  test("起動しただけでは削らない", () => {
    const list = fakeList(CHAT_ITEM_LIMIT + 10);
    const scheduler = fakeScheduler();

    startChatTrim(() => list, scheduler.schedule);

    expect(list.removed).toBe(0);
  });

  test("周期処理が回るたびに上限を当てる", () => {
    const list = fakeList(CHAT_ITEM_LIMIT + 10);
    const scheduler = fakeScheduler();
    startChatTrim(() => list, scheduler.schedule);

    scheduler.run();

    expect(list.childElementCount).toBe(CHAT_ITEM_LIMIT);
  });

  test("定めた間隔で回すよう登録する", () => {
    const scheduler = fakeScheduler();

    startChatTrim(() => null, scheduler.schedule);

    expect(scheduler.intervalMs).toBe(CHAT_TRIM_INTERVAL_MS);
  });

  /**
   * 適用先の要素は Polymer が後から作り、遷移でも作り直される。掴んだ参照を持ち越すと
   * 差し替え後の要素に効かなくなるので、周期ごとに引き直していることを固定する。
   */
  test("適用先を周期ごとに引き直す", () => {
    const scheduler = fakeScheduler();
    const lists = [null, fakeList(CHAT_ITEM_LIMIT + 5)] as const;
    let lookups = 0;
    startChatTrim(() => lists[lookups++] ?? null, scheduler.schedule);

    scheduler.run();
    scheduler.run();

    expect(lookups).toBe(2);
    expect(lists[1].childElementCount).toBe(CHAT_ITEM_LIMIT);
  });
});
