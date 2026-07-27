import { describe, expect, test } from "bun:test";

import { surfaceOf } from "../src/main/surface";

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
