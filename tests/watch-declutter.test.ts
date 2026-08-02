import { describe, expect, test } from "bun:test";

import {
  COMMENTS_GROUP,
  NEXT_VIDEOS_GROUP,
  type ElementRoot,
} from "../src/isolated/declutter";
import {
  hasAddedNodes,
  installWatchDeclutter,
} from "../src/isolated/watch-declutter";
import { onNavigated } from "../src/shared/navigation";
import type { SettingsStore, StoredChange } from "../src/shared/settings";

import { flush } from "./support/flush";
import { fakeNavigationSource } from "./support/navigation-source";

/**
 * watch ページの整理の繋ぎ込み（設定・実 DOM・再適用の契機）。
 *
 * 何を消すかは declutter が持ち、そちらのテストで固定してある。ここで固定するのは
 * 「いつ消すか」だけで、最重要は最初の適用が保存値の読み出しより後に来ること。
 * 消したノードは戻せないので、既定（消す）を保存値より先に当てると「残す」を選んでいる人の
 * コメント欄が失われる。この順序は型でも実行時の失敗でも表に出ないため、テストでしか守れない。
 */

/** 消された塊の名前を記録する根。実 DOM ではなくセレクタと remove の呼び出しだけを見る。 */
const recordingRoot = (): {
  root: ElementRoot;
  readonly removedGroups: () => readonly string[];
} => {
  const removedSelectors: string[] = [];
  return {
    root: {
      querySelectorAll: (selectors) => [
        {
          remove: () => {
            removedSelectors.push(selectors);
          },
        },
      ],
    },
    removedGroups: () =>
      [NEXT_VIDEOS_GROUP, COMMENTS_GROUP]
        .filter((group) =>
          group.selectors.some((selector) => removedSelectors.includes(selector)),
        )
        .map((group) => group.name),
  };
};

/**
 * 読み出しの解決をテストが握る storage。
 *
 * 「保存値が届く前」を作れることがこのフェイクの目的なので、get は resolve するまで待たせる。
 */
const controlledStore = (): {
  store: SettingsStore;
  resolve: (stored: unknown) => Promise<void>;
  change: (newValue: unknown) => void;
} => {
  const listeners = new Set<(changes: Record<string, StoredChange>) => void>();
  let settle: ((stored: Record<string, unknown>) => void) | undefined;
  return {
    store: {
      get: () =>
        new Promise<Record<string, unknown>>((resolve) => {
          settle = resolve;
        }),
      set: async () => {},
      onChanged: {
        addListener: (listener) => {
          listeners.add(listener);
        },
        removeListener: (listener) => {
          listeners.delete(listener);
        },
      },
      // 失効はこのフェイクの関心の外。失効の扱いは shared/settings のテストが見る。
      isAlive: () => true,
    },
    resolve: async (stored) => {
      settle?.({ watchDeclutter: stored });
      await flush();
    },
    change: (newValue) => {
      for (const listener of listeners) listener({ watchDeclutter: { newValue } });
    },
  };
};

/** 登録されたコールバックをテストから発火させる契機。登録が無ければ発火しても何も起きない。 */
const capturedTrigger = (): {
  register: (callback: () => void) => void;
  fire: () => void;
} => {
  const callbacks: (() => void)[] = [];
  return {
    register: (callback) => {
      callbacks.push(callback);
    },
    fire: () => {
      for (const callback of callbacks) callback();
    },
  };
};

/** 待ち時間つきの処理をテストから即座に走らせるための保留箱。 */
const capturedDelay = (): {
  delay: (task: () => void, delayMs: number) => void;
  run: () => void;
  readonly delays: readonly number[];
} => {
  const tasks: (() => void)[] = [];
  const delays: number[] = [];
  return {
    delay: (task, delayMs) => {
      tasks.push(task);
      delays.push(delayMs);
    },
    run: () => {
      for (const task of tasks) task();
    },
    get delays() {
      return delays;
    },
  };
};

/**
 * 遷移は実物の onNavigated を通す。
 *
 * 初回適用が走る条件（readyState が loading なら DOMContentLoaded まで待つ）は onNavigated が
 * 持っており、自前の「登録したら 1 回呼ぶ」フェイクで代用するとその条件が検査から抜ける。
 * 既定を "complete" にしてあるのは、storage の読み出しが解決する頃には構築が進んでいる場合を
 * 既定の状況として扱うため。document_start に居合わせた場合は個別のテストで指定する。
 */
const install = (readyState: DocumentReadyState = "complete") => {
  const dom = recordingRoot();
  const storage = controlledStore();
  const additions = capturedTrigger();
  const navigation = fakeNavigationSource(readyState);
  const timer = capturedDelay();
  const reported: string[] = [];

  installWatchDeclutter({
    store: storage.store,
    root: dom.root,
    watchAdditions: additions.register,
    navigate: (apply) => onNavigated(apply, navigation.source),
    delay: timer.delay,
    report: (message) => reported.push(message),
  });

  return { dom, storage, additions, navigation, timer, reported };
};

describe("installWatchDeclutter の適用順序", () => {
  // 最重要。既定は「消す」なので、読み出しより先に当たると「残す」設定の人のコメント欄が消える。
  test("保存値の読み出しが解決するまでは何も消さない", () => {
    const { dom, additions, navigation } = install();

    // 契機が先に張られていれば、この発火で既定値のまま適用が走る。
    navigation.dispatch("yt-navigate-finish");
    navigation.dispatch("DOMContentLoaded");
    additions.fire();

    expect(dom.removedGroups()).toEqual([]);
  });

  test("読み出しが解決してから初めて消す", async () => {
    const { dom, storage } = install();

    await storage.resolve(undefined);

    expect(dom.removedGroups()).toEqual(["next-videos", "comments"]);
  });

  /**
   * 読み出しが構築中に解決した場合。
   *
   * onNavigated は readyState が loading のあいだ初回適用を DOMContentLoaded まで待たせる。
   * 消す対象はまだ挿入されていないので待って構わないが、待ったぶんが取りこぼしにならないこと
   * （後から来る DOMContentLoaded で消える）は確かめておく。
   */
  test("構築中に読み出しが解決したら、DOMContentLoaded まで待ってから消す", async () => {
    const { dom, storage, navigation } = install("loading");

    await storage.resolve(undefined);
    expect(dom.removedGroups()).toEqual([]);

    navigation.dispatch("DOMContentLoaded");

    expect(dom.removedGroups()).toEqual(["next-videos", "comments"]);
  });

  // 「残す」を選んでいる人のコメント欄は、どの契機を通しても消えてはならない。
  test("コメント欄を残す設定なら、遷移でも DOM の追加でもコメント欄は消えない", async () => {
    const { dom, storage, additions, navigation } = install();

    navigation.dispatch("yt-navigate-finish");
    additions.fire();
    await storage.resolve({ removeComments: false });

    navigation.dispatch("yt-navigate-finish");
    additions.fire();

    expect(dom.removedGroups()).toEqual(["next-videos"]);
  });
});

describe("installWatchDeclutter の再適用", () => {
  test("DOM への追加があれば消し直す", async () => {
    const { dom, storage, additions } = install();
    await storage.resolve({ removeComments: false });

    additions.fire();

    expect(dom.removedGroups()).toEqual(["next-videos"]);
  });

  test("設定が「消す」へ変わったらその場で消す", async () => {
    const { dom, storage } = install();
    await storage.resolve({ removeComments: false });

    storage.change({ removeComments: true });

    expect(dom.removedGroups()).toEqual(["next-videos", "comments"]);
  });
});

describe("installWatchDeclutter の腐食報告", () => {
  test("消せなかった塊があれば待ち時間の後に報告する", async () => {
    const dom: ElementRoot = { querySelectorAll: () => [] };
    const storage = controlledStore();
    const timer = capturedDelay();
    const reported: string[] = [];
    installWatchDeclutter({
      store: storage.store,
      root: dom,
      watchAdditions: () => {},
      navigate: (apply) => apply(),
      delay: timer.delay,
      report: (message) => reported.push(message),
    });

    await storage.resolve(undefined);

    timer.run();

    expect(reported).toHaveLength(1);
    expect(reported[0]).toContain("next-videos");
    expect(reported[0]).toContain("comments");
  });

  test("すべて消せていれば報告しない", async () => {
    const { storage, timer, reported } = install();

    await storage.resolve(undefined);

    timer.run();

    expect(reported).toEqual([]);
  });

  // 報告は読み出しの後に仕掛ける。先に仕掛けると、まだ設定が決まっていない塊の名前が出る。
  test("報告を仕掛けるのは読み出しが解決した後", async () => {
    const { storage, timer } = install();

    expect(timer.delays).toEqual([]);

    await storage.resolve(undefined);

    expect(timer.delays).toHaveLength(1);
  });
});

describe("hasAddedNodes", () => {
  // 追加が無い変更でも探し直すと、自分の削除が次の探索を呼ぶ連鎖になる。
  test("追加を含む変更なら true", () => {
    expect(hasAddedNodes([{ addedNodes: { length: 0 } }, { addedNodes: { length: 2 } }])).toBe(
      true,
    );
  });

  test("削除だけの変更なら false", () => {
    expect(hasAddedNodes([{ addedNodes: { length: 0 } }])).toBe(false);
  });

  test("変更が空なら false", () => {
    expect(hasAddedNodes([])).toBe(false);
  });
});
