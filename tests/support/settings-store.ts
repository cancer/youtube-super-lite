import type { SettingsStore, StoredChange } from "../../src/shared/settings";

/**
 * chrome.storage.local のフェイク。
 *
 * 保存済みの生データを直接覗ける形にしてあるのは、「読み出しで正規化されている」ことと
 * 「保存値そのものは触っていない」ことを別々に検証するため。
 */
export const fakeStore = (
  stored: Record<string, unknown> = {},
): { store: SettingsStore; stored: Record<string, unknown> } => {
  const listeners = new Set<(changes: Record<string, StoredChange>) => void>();
  const store: SettingsStore = {
    get: async (keys) =>
      Object.fromEntries(
        keys.filter((key) => key in stored).map((key) => [key, stored[key]]),
      ),
    set: async (items) => {
      Object.assign(stored, items);
      const changes = Object.fromEntries(
        Object.entries(items).map(([key, value]) => [key, { newValue: value }]),
      );
      for (const listener of listeners) listener(changes);
    },
    onChanged: {
      addListener: (listener) => {
        listeners.add(listener);
      },
      removeListener: (listener) => {
        listeners.delete(listener);
      },
    },
  };
  return { store, stored };
};
