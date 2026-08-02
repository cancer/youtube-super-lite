import { describe, expect, test } from "bun:test";

import {
  COMMENTS_GROUP,
  NEXT_VIDEOS_GROUP,
  applyDeclutter,
  removalGroupsFor,
  unmatchedGroupNames,
  type ElementRoot,
} from "../src/isolated/declutter";

/**
 * document のフェイク。セレクタごとに「そのセレクタで見つかるノード」を並べて渡す。
 *
 * 実際の CSS マッチングは持たない。ここで確かめたいのは「宣言したセレクタで見つかったものだけを
 * 消す」ことであり、セレクタ文字列が YouTube の実 DOM に当たるかは別（実ブラウザでしか確かめられない）。
 * 削除済みのノードは以降どのセレクタでも見つからない。実 DOM で親を消せば子も外れることと、
 * 同じ塊を 2 度当てても二重に消えないことを、この 1 点で表す。
 */
const fakeRoot = (
  matches: Record<string, readonly string[]>,
): { root: ElementRoot; removed: string[] } => {
  const removed: string[] = [];
  const root: ElementRoot = {
    querySelectorAll: (selectors) =>
      (matches[selectors] ?? [])
        .filter((id) => !removed.includes(id))
        .map((id) => ({
          remove: () => {
            removed.push(id);
          },
        })),
  };
  return { root, removed };
};

/** 実 DOM に居るはずの全対象を並べたフェイク。「消しすぎ」を見るため周辺の要素も置く。 */
const watchPage = (): { root: ElementRoot; removed: string[] } =>
  fakeRoot({
    ...Object.fromEntries(
      NEXT_VIDEOS_GROUP.selectors.map((selector) => [selector, ["次の動画の列"]]),
    ),
    ...Object.fromEntries(
      COMMENTS_GROUP.selectors.map((selector, index) => [
        selector,
        [`コメント欄${index}`],
      ]),
    ),
    "ytd-watch-flexy ytd-watch-metadata": ["タイトルとチャンネル情報と高評価"],
  });

describe("applyDeclutter", () => {
  test("コメント欄を消す設定なら、コメント欄を消す", () => {
    const { root, removed } = watchPage();

    applyDeclutter(root, { removeComments: true });

    expect(removed).toEqual(
      expect.arrayContaining(
        COMMENTS_GROUP.selectors.map((_, index) => `コメント欄${index}`),
      ),
    );
  });

  test("コメント欄を残す設定なら、コメント欄を消さない", () => {
    const { root, removed } = watchPage();

    applyDeclutter(root, { removeComments: false });

    expect(removed).toEqual(["次の動画の列"]);
  });

  test("コメント欄を消す設定でも「次の動画」の列を消す", () => {
    const { root, removed } = watchPage();

    applyDeclutter(root, { removeComments: true });

    expect(removed).toContain("次の動画の列");
  });

  test("コメント欄を残す設定でも「次の動画」の列を消す", () => {
    const { root, removed } = watchPage();

    applyDeclutter(root, { removeComments: false });

    expect(removed).toContain("次の動画の列");
  });

  test("名指ししていない要素は消さない", () => {
    const { root, removed } = watchPage();

    applyDeclutter(root, { removeComments: true });

    expect(removed).not.toContain("タイトルとチャンネル情報と高評価");
  });

  test("対象が 1 つも無くても例外を投げない", () => {
    const { root, removed } = fakeRoot({});

    expect(() => applyDeclutter(root, { removeComments: true })).not.toThrow();
    expect(removed).toEqual([]);
  });

  test("対象が 1 つも無ければ、消せた塊は無いと報告する", () => {
    const { root } = fakeRoot({});

    expect(applyDeclutter(root, { removeComments: true })).toEqual([]);
  });

  test("消せた塊の名前を返す", () => {
    const { root } = watchPage();

    expect(applyDeclutter(root, { removeComments: true })).toEqual([
      NEXT_VIDEOS_GROUP.name,
      COMMENTS_GROUP.name,
    ]);
  });

  test("2 回目の適用では消すものが無く、消した数も増えない", () => {
    const { root, removed } = watchPage();
    applyDeclutter(root, { removeComments: true });
    const afterFirst = [...removed];

    const secondResult = applyDeclutter(root, { removeComments: true });

    expect(removed).toEqual(afterFirst);
    expect(secondResult).toEqual([]);
  });

  test("遷移で対象が作り直されたら、再適用で消し直す", () => {
    const { root, removed } = watchPage();
    applyDeclutter(root, { removeComments: true });
    const rebuilt = watchPage();

    applyDeclutter(rebuilt.root, { removeComments: true });

    expect(rebuilt.removed).toEqual(removed);
  });
});

describe("removalGroupsFor", () => {
  test("コメント欄を消す設定なら両方の塊を消す", () => {
    expect(removalGroupsFor({ removeComments: true })).toEqual([
      NEXT_VIDEOS_GROUP,
      COMMENTS_GROUP,
    ]);
  });

  test("コメント欄を残す設定なら「次の動画」の列だけを消す", () => {
    expect(removalGroupsFor({ removeComments: false })).toEqual([
      NEXT_VIDEOS_GROUP,
    ]);
  });
});

describe("unmatchedGroupNames", () => {
  test("1 つも消せていなければ、消すはずの塊をすべて挙げる", () => {
    expect(unmatchedGroupNames({ removeComments: true }, [])).toEqual([
      NEXT_VIDEOS_GROUP.name,
      COMMENTS_GROUP.name,
    ]);
  });

  test("消せた塊は挙げない", () => {
    expect(
      unmatchedGroupNames({ removeComments: true }, [NEXT_VIDEOS_GROUP.name]),
    ).toEqual([COMMENTS_GROUP.name]);
  });

  test("消さない設定の塊は、消せていなくても挙げない", () => {
    expect(unmatchedGroupNames({ removeComments: false }, [])).toEqual([
      NEXT_VIDEOS_GROUP.name,
    ]);
  });

  test("すべて消せていれば何も挙げない", () => {
    expect(
      unmatchedGroupNames({ removeComments: true }, [
        NEXT_VIDEOS_GROUP.name,
        COMMENTS_GROUP.name,
      ]),
    ).toEqual([]);
  });
});

/**
 * セレクタ自体の取り違えは実 DOM でしか見つけられないので、ここでは
 * 「実ブラウザで確かめた文字列から勝手にずれていないか」だけを固定する。
 *
 * 出所: 2026-08-01 に実ブラウザ（未ログイン）の watch ページで確認した DOM。
 * 「次の動画」の列は 2 カラム時に `#secondary-inner > #related`、1 カラム時に
 * `#primary-inner > #below > div > #related` へ移るため、列ではなく `#related` を名指しする。
 * ライブチャット（`#chat-container`）・再生リスト（`#playlist`）・タイトルとチャンネル情報
 * （`ytd-watch-metadata`）はいずれも `#related` の外にあるので巻き添えにならない。
 */
describe("削除対象のセレクタ", () => {
  test("「次の動画」の列は #related だけを名指しする", () => {
    expect(NEXT_VIDEOS_GROUP.selectors).toEqual(["ytd-watch-flexy #related"]);
  });

  test("コメント欄はコメント専用の 3 か所を名指しする", () => {
    expect(COMMENTS_GROUP.selectors).toEqual([
      "ytd-watch-flexy ytd-comments#comments",
      "ytd-watch-flexy #comments-leave-behind",
      'ytd-watch-flexy ytd-engagement-panel-section-list-renderer[target-id="engagement-panel-comments-section"]',
    ]);
  });
});
