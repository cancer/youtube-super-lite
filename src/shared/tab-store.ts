import type { SettingsStore, StoredChange } from "./settings";

/**
 * 設定をタブごとに分ける。
 *
 * 分ける仕組みを区画（SettingsSection）ではなく保存先（SettingsStore）に置いてある。区画の側で
 * 分けると、区画のキーがタブ番号を含んだ別物になり、そのキーを外へ出す経路（MAIN world への
 * 配送）まで巻き込む。保存先の側なら、設定を使う機能は「どの保存先から読むか」だけが変わり、
 * 区画の名前も配送の中身も動かない。
 */

/**
 * タブ単位のキーの前置き。
 *
 * 区切りを含めた形で持つのは、後始末（タブが閉じたときの削除）が前方一致で判定するため。
 * 区切りが無いと tab4 の前置きが tab42 のキーにも当たる。
 */
export const tabKeyPrefix = (tabId: number): string => `tab${tabId}:`;

/** 区画のキーを、そのタブだけのキーにする。 */
export const tabKey = (tabId: number, key: string): string =>
  `${tabKeyPrefix(tabId)}${key}`;

/** 与えたキーのうち、そのタブのものだけを返す。タブが閉じたときの後始末に使う。 */
export const keysOfTab = (
  keys: readonly string[],
  tabId: number,
): readonly string[] => keys.filter((key) => key.startsWith(tabKeyPrefix(tabId)));

/**
 * 保存先をタブ 1 つ分に閉じる。
 *
 * 読み書きではキーを前置きの付いたものへ、変更通知では逆に前置きを外したものへ戻す。
 * これで、この保存先を渡された側は自分がタブ単位で動いていることを知らずに済む。
 * 他のタブのキーは変更通知の段階で落ちるので、隣のタブの変更で反応することもない。
 */
export const tabScopedStore = (
  store: SettingsStore,
  tabId: number,
): SettingsStore => {
  const prefix = tabKeyPrefix(tabId);
  const scoped = (key: string): string => `${prefix}${key}`;
  /** このタブのキーなら元の区画のキーへ戻す。他のタブや無関係のキーなら undefined。 */
  const unscoped = (key: string): string | undefined =>
    key.startsWith(prefix) ? key.slice(prefix.length) : undefined;

  /**
   * 購読の解除には登録したのと同じ関数が要る。呼び出し側が渡した関数と、実際に登録した
   * 包み直しの関数との対応をここで持つ。
   */
  const wrapped = new Map<
    (changes: Record<string, StoredChange>) => void,
    (changes: Record<string, StoredChange>) => void
  >();

  return {
    get: async (keys) => {
      const values = await store.get(keys.map(scoped));
      return Object.fromEntries(
        Object.entries(values).flatMap(([key, value]) => {
          const original = unscoped(key);
          return original === undefined ? [] : [[original, value] as const];
        }),
      );
    },
    set: (items) =>
      store.set(
        Object.fromEntries(
          Object.entries(items).map(([key, value]) => [scoped(key), value]),
        ),
      ),
    onChanged: {
      addListener: (listener) => {
        const relay = (changes: Record<string, StoredChange>): void => {
          const own = Object.entries(changes).flatMap(([key, change]) => {
            const original = unscoped(key);
            return original === undefined ? [] : [[original, change] as const];
          });
          // 自分のタブの変更が 1 つも無ければ呼ばない。無関係な変更で購読側を起こさないため。
          if (own.length === 0) return;
          listener(Object.fromEntries(own));
        };
        wrapped.set(listener, relay);
        store.onChanged.addListener(relay);
      },
      removeListener: (listener) => {
        const relay = wrapped.get(listener);
        if (relay === undefined) return;
        wrapped.delete(listener);
        store.onChanged.removeListener(relay);
      },
    },
    isAlive: () => store.isAlive(),
  };
};
