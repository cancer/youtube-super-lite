import { describe, expect, test } from "bun:test";

import { DELIVERED_SECTIONS, startSettingsDelivery } from "../src/isolated/deliver";
import { watchDeclutterSection, writeSection } from "../src/shared/settings";
import type { SettingsSection } from "../src/shared/settings";

import { fakeStore } from "./support/settings-store";

/**
 * ISOLATED world から MAIN world への設定配送の繋ぎ込み。
 *
 * 配る中身の組み立てと受け取りは bridge が持ち、そちらのテストで固定してある。
 * ここで固定するのは「いつ・どの区画を配るか」だけ。
 */

/** 配送を記録する送り先。実体は window.postMessage 経由の publishSection。 */
const recordingPublisher = (): {
  publish: <T>(section: SettingsSection<T>, value: T) => void;
  readonly delivered: ReadonlyArray<[string, unknown]>;
} => {
  const delivered: [string, unknown][] = [];
  return {
    publish: (section, value) => {
      delivered.push([section.key, value]);
    },
    get delivered() {
      return delivered;
    },
  };
};

/**
 * 遷移の契機。
 *
 * 登録時には呼ばない。この content script は document_start で走るので、onNavigated は初回を
 * DOMContentLoaded まで遅らせる。そこを待たずに配ることが起動時の配送の役目なので、
 * フェイクもその状況（登録しただけでは何も起きない）を写す。
 */
const capturedNavigation = (): {
  navigate: (apply: () => void) => void;
  fire: () => void;
} => {
  const callbacks: (() => void)[] = [];
  return {
    navigate: (apply) => {
      callbacks.push(apply);
    },
    fire: () => {
      for (const apply of callbacks) apply();
    },
  };
};

/** 配送は非同期の読み出しを挟むので、検査の前に片付くまで待つ。 */
const flush = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

describe("配送対象の区画", () => {
  /**
   * 配送経路はページの JS から観測できるので、載せる区画は MAIN world が要る分だけに絞る。
   * 保存されている全区画を配ってしまう変更をここで止める。
   */
  test("保存されている全区画を配るのではない（watch ページの整理は載せない）", () => {
    expect(DELIVERED_SECTIONS.map((section) => section.key)).not.toContain(
      watchDeclutterSection.key,
    );
  });
});

describe("startSettingsDelivery", () => {
  const start = (stored: Record<string, unknown> = {}) => {
    const { store } = fakeStore(stored);
    const publisher = recordingPublisher();
    const navigation = capturedNavigation();
    startSettingsDelivery({
      store,
      publish: publisher.publish,
      navigate: navigation.navigate,
    });
    return { store, publisher, navigation };
  };

  test("遷移を待たずに、起動時に配送対象の区画をすべて配る", async () => {
    const { publisher } = start();

    await flush();

    expect(publisher.delivered.map(([key]) => key)).toEqual(
      DELIVERED_SECTIONS.map((section) => section.key),
    );
  });

  test("保存値を正規化してから配る", async () => {
    const [section] = DELIVERED_SECTIONS;
    const { publisher } = start({ [section.key]: "壊れた値" });

    await flush();

    expect(publisher.delivered[0]).toEqual([section.key, section.defaults]);
  });

  test("区画が変わったら配り直す", async () => {
    const [section] = DELIVERED_SECTIONS;
    const { store, publisher } = start();
    await flush();

    await writeSection(store, section, section.defaults);

    expect(publisher.delivered.slice(DELIVERED_SECTIONS.length)).toEqual([
      [section.key, section.defaults],
    ]);
  });

  // 両 world の注入順は保証されないので初回の配送は取りこぼし得る。遷移のたびに配り直して追いつく。
  test("遷移のたびに全区画を配り直す", async () => {
    const { publisher, navigation } = start();
    await flush();

    navigation.fire();
    await flush();

    expect(publisher.delivered).toHaveLength(DELIVERED_SECTIONS.length * 2);
  });
});
