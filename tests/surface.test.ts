import { describe, expect, test } from "bun:test";

import { surfaceOf, type Surface } from "../src/shared/surface";

import { injectedPathnames } from "./support/manifest";

describe("surfaceOf", () => {
  test("watch ページを watch と判定する", () => {
    expect(surfaceOf("/watch")).toBe("watch");
  });

  test("ライブチャットの iframe を live_chat と判定する", () => {
    expect(surfaceOf("/live_chat")).toBe("live_chat");
  });

  test("ライブチャットのリプレイも live_chat と判定する", () => {
    expect(surfaceOf("/live_chat_replay")).toBe("live_chat");
  });

  test("いずれでもない pathname は other と判定する", () => {
    expect(surfaceOf("/feed/subscriptions")).toBe("other");
  });
});

/**
 * manifest とコードの対応。
 *
 * どこに注入するかは manifest の宣言が決め、注入先で何をするかはコードが決める。宣言は
 * コードから参照できないので、両者がズレたことに気づく手立てはこの検査しかない。
 * ズレると「注入されているのに other 扱いで何も動かない面」や「判定だけあって注入されない面」が
 * 静かに生まれる。
 */
describe("manifest の注入先との対応", () => {
  test("manifest が注入する面はすべて surfaceOf が名前を持つ", () => {
    const unnamed = injectedPathnames().filter(
      (pathname) => surfaceOf(pathname) === "other",
    );

    expect(unnamed).toEqual([]);
  });

  test("surfaceOf が名前を持つ面と manifest の注入先が過不足なく対応する", () => {
    const named: readonly Surface[] = ["watch", "live_chat"];

    expect(new Set(injectedPathnames().map(surfaceOf))).toEqual(new Set(named));
  });
});
