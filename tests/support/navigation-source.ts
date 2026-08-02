import type { NavigationSource } from "../../src/shared/navigation";

/**
 * 遷移イベントの発火元（document）のフェイク。
 *
 * onNavigated を素通しさせずに自前の「登録したら 1 回呼ぶ」フェイクで代用すると、初回適用の
 * 条件（readyState が loading なら DOMContentLoaded まで待つ）が検査から抜け落ちる。
 * ここは実物の onNavigated を通せる形にして、その条件ごと本物を使う。
 */
export const fakeNavigationSource = (
  readyState: DocumentReadyState,
): {
  source: NavigationSource;
  dispatch: (type: string) => void;
} => {
  const listeners = new Map<string, Set<() => void>>();
  return {
    source: {
      readyState,
      addEventListener: (type, listener, options) => {
        const once = options?.once === true;
        const wrapped = once
          ? (): void => {
              listeners.get(type)?.delete(wrapped);
              listener();
            }
          : listener;
        const forType = listeners.get(type) ?? new Set<() => void>();
        forType.add(wrapped);
        listeners.set(type, forType);
      },
      removeEventListener: (type, listener) => {
        listeners.get(type)?.delete(listener);
      },
    },
    dispatch: (type) => {
      for (const listener of [...(listeners.get(type) ?? [])]) listener();
    },
  };
};
