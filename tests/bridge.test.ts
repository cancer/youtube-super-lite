import { describe, expect, test } from "bun:test";

import {
  isSameFrameDelivery,
  settingFromMessage,
  settingsMessage,
} from "../src/shared/bridge";
import { chatDisplaySection } from "../src/shared/settings";

describe("isSameFrameDelivery", () => {
  const frame = {};
  const origin = "https://www.youtube.com";

  test("同一フレーム・同一オリジンからの message を受ける", () => {
    expect(isSameFrameDelivery({ source: frame, origin }, frame, origin)).toBe(
      true,
    );
  });

  test("別フレームからの message を弾く", () => {
    expect(isSameFrameDelivery({ source: {}, origin }, frame, origin)).toBe(
      false,
    );
  });

  test("source を持たない message を弾く", () => {
    expect(isSameFrameDelivery({ source: null, origin }, frame, origin)).toBe(
      false,
    );
  });

  test("別オリジンからの message を弾く", () => {
    expect(
      isSameFrameDelivery(
        { source: frame, origin: "https://evil.example" },
        frame,
        origin,
      ),
    ).toBe(false);
  });
});

describe("settingFromMessage", () => {
  test("自分が組んだ配送メッセージから設定を復元する", () => {
    const value = { fontSizePx: 22, panelWidthRatio: 0.4 };

    expect(
      settingFromMessage(
        chatDisplaySection,
        settingsMessage(chatDisplaySection, value),
      ),
    ).toEqual(value);
  });

  test("名前空間が違うメッセージは無視する", () => {
    expect(
      settingFromMessage(chatDisplaySection, {
        type: "settings",
        key: chatDisplaySection.key,
        value: { fontSizePx: 22, panelWidthRatio: 0.4 },
      }),
    ).toBeUndefined();
  });

  test("名前空間が一致しても別区画のキーなら無視する", () => {
    expect(
      settingFromMessage(chatDisplaySection, {
        ...settingsMessage(chatDisplaySection, chatDisplaySection.defaults),
        key: "equalizer",
      }),
    ).toBeUndefined();
  });

  test("ページが投げる無関係なメッセージを無視する", () => {
    expect(settingFromMessage(chatDisplaySection, "yt-player-ready")).toBeUndefined();
  });

  test("null を無視する", () => {
    expect(settingFromMessage(chatDisplaySection, null)).toBeUndefined();
  });

  test("キーの無いメッセージを無視する", () => {
    expect(
      settingFromMessage(chatDisplaySection, {
        type: settingsMessage(chatDisplaySection, chatDisplaySection.defaults).type,
      }),
    ).toBeUndefined();
  });

  test("偽装された範囲外の値は範囲内へ収めて渡す", () => {
    expect(
      settingFromMessage(chatDisplaySection, {
        ...settingsMessage(chatDisplaySection, chatDisplaySection.defaults),
        value: { fontSizePx: 400, panelWidthRatio: 9 },
      }),
    ).toEqual({ fontSizePx: 28, panelWidthRatio: 0.6 });
  });
});
