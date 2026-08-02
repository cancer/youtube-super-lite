import type { SettingsStore, StoredChange } from "../../src/shared/settings";

/**
 * chrome.storage.local のフェイク。
 *
 * 保存済みの生データを直接覗ける形にしてあるのは、「読み出しで正規化されている」ことと
 * 「保存値そのものは触っていない」ことを別々に検証するため。
 *
 * invalidate は拡張コンテキストの失効を起こす。実物では拡張の再読み込みで storage への
 * 操作が失敗するようになり、同時に生存の判定材料も消えるので、フェイクも両方を一度に変える。
 * 片方だけを起こせる形にすると、実物では起きない組み合わせをテストが固定してしまう。
 */
export const fakeStore = (
  stored: Record<string, unknown> = {},
): {
  store: SettingsStore;
  stored: Record<string, unknown>;
  invalidate: () => void;
} => {
  const listeners = new Set<(changes: Record<string, StoredChange>) => void>();
  let alive = true;
  const refuseWhenInvalidated = (): void => {
    if (!alive) throw new Error("Extension context invalidated.");
  };
  const store: SettingsStore = {
    get: async (keys) => {
      refuseWhenInvalidated();
      return Object.fromEntries(
        keys.filter((key) => key in stored).map((key) => [key, stored[key]]),
      );
    },
    set: async (items) => {
      refuseWhenInvalidated();
      Object.assign(stored, items);
      const changes = Object.fromEntries(
        Object.entries(items).map(([key, value]) => [key, { newValue: value }]),
      );
      for (const listener of listeners) listener(changes);
    },
    onChanged: {
      addListener: (listener) => {
        refuseWhenInvalidated();
        listeners.add(listener);
      },
      removeListener: (listener) => {
        refuseWhenInvalidated();
        listeners.delete(listener);
      },
    },
    isAlive: () => alive,
  };
  return {
    store,
    stored,
    invalidate: () => {
      alive = false;
    },
  };
};
