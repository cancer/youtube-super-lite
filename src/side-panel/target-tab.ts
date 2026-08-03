import { PERSISTED_SECTIONS } from "../shared/sections";
import {
  applySection,
  localSettingsStore,
  patchSection,
  seedSection,
  sessionSettingsStore,
  watchSection,
  writeSection,
  type SettingsSection,
  type SettingsStore,
} from "../shared/settings";
import { tabScopedStore } from "../shared/tab-store";

/**
 * サイドパネルが相手にするタブ。
 *
 * パネルはウィンドウに 1 つで、タブを切り替えても開いたまま残る。操作の相手は「今見ているタブ」
 * ひとつに限る。開いている他のタブは、こちらで何を変えても動かない。
 *
 * この決まりごとを守る場所をここ 1 つにしてあるので、操作 UI の側（機能ごとのモジュール）は
 * 相手がどのタブかも、保存先が切り替わることも知らずに書ける。
 */

/** タブ 1 つ。番号だけでなくウィンドウも見るのは、他のウィンドウでの切り替えに反応しないため。 */
export type ActiveTab = {
  readonly tabId: number;
  readonly windowId: number;
};

/** 繋ぎ先。既定は実ブラウザのもので、テストはすべて差し替える。 */
export type TargetTabOptions = {
  /** このパネルのウィンドウで今見ているタブ。分からなければ undefined。 */
  readonly activeTab: () => Promise<ActiveTab | undefined>;
  /** 見ているタブが変わったときに呼ばれる（どのウィンドウのぶんも来る）。 */
  readonly onActivated: (listener: (tab: ActiveTab) => void) => void;
  /** タブ 1 つ分の保存先。 */
  readonly storeOfTab: (tabId: number) => SettingsStore;
  /** 永続の保存先。パネルでの変更は、次に開くタブの初期値としてこちらにも残す。 */
  readonly persistent: SettingsStore;
  /** タブごとに分ける区画。相手のタブが決まるたび、未設定のぶんを永続の保存値で埋める。 */
  readonly sections: readonly SettingsSection<unknown>[];
};

/**
 * 相手のタブへ張り直す配線 1 つ。戻り値は購読の解除。
 *
 * 区画の型を関数の中へ閉じ込めてあるので、この一覧は区画ごとの型を持ち回らずに済む。
 */
type Follower = (store: SettingsStore, ticket: number) => () => void;

export type TargetTab = {
  /**
   * 区画の値を描く。相手のタブが決まったとき・変わったとき・そのタブで値が変わったときに呼ばれる。
   * 登録は相手が決まる前でよい。
   */
  readonly follow: <T>(
    section: SettingsSection<T>,
    render: (value: T) => void,
  ) => void;
  /** 区画の値を相手のタブへ当て、次に開くタブの初期値としても残す。 */
  readonly apply: <T>(section: SettingsSection<T>, value: T) => Promise<void>;
  /**
   * 区画のうち渡したフィールドだけを相手のタブへ当て、次に開くタブの初期値としても残す。
   *
   * 区画を複数の操作面で分担しているとき（R5 の幅はページ内のハンドルが持つ）はこちらを使う。
   * apply だと、パネルが持たないフィールドまで自分の手元の値で上書きしてしまう。
   */
  readonly patch: <T>(
    section: SettingsSection<T>,
    patch: Partial<T>,
  ) => Promise<void>;
  /** 相手を探し始める。操作 UI の登録がすべて済んでから呼ぶ。 */
  readonly start: () => Promise<void>;
};

export const createTargetTab = ({
  activeTab,
  onActivated,
  storeOfTab,
  persistent,
  sections,
}: TargetTabOptions): TargetTab => {
  const followers: Follower[] = [];
  let unsubscribes: (() => void)[] = [];
  let store: SettingsStore | undefined;
  let windowId: number | undefined;
  /**
   * 相手が変わった回数。
   *
   * 読み出しは非同期なので、切り替えが続くと前の相手の値が後から届き得る。届いた時点で
   * 相手が変わっていれば捨てる。捨てないと、パネルに今見ていないタブの値が残る。
   */
  let generation = 0;

  const attach = (follower: Follower, ticket: number): void => {
    if (store === undefined) return;
    unsubscribes.push(follower(store, ticket));
  };

  const retarget = async (tabId: number): Promise<void> => {
    const ticket = ++generation;
    const next = storeOfTab(tabId);
    // まだ何も設定していないタブは、永続の保存値で埋めてから読む。埋めずに読むと、
    // そのタブが実際に使っている値（同じく永続の保存値から起きる）とパネルの表示がずれる。
    for (const section of sections) await seedSection(persistent, next, section);
    if (ticket !== generation) return;
    for (const unsubscribe of unsubscribes) unsubscribe();
    unsubscribes = [];
    store = next;
    for (const follower of followers) attach(follower, ticket);
  };

  return {
    follow: (section, render) => {
      const follower: Follower = (attached, ticket) => {
        // 読み出しの結果は後から届く。届いた時点で相手が変わっていれば描かない。
        void applySection(attached, section, (value) => {
          if (ticket === generation) render(value);
        });
        return watchSection(attached, section, render);
      };
      followers.push(follower);
      attach(follower, generation);
    },

    apply: async (section, value) => {
      // 今見ているタブへ先に当てる。永続の保存値は他のタブの今には影響しないので後でよい。
      if (store !== undefined && store !== persistent) {
        await writeSection(store, section, value);
      }
      await writeSection(persistent, section, value);
    },

    patch: async (section, patch) => {
      // 当てる順序と相手は apply と同じ。違いは「区画ごと」か「フィールドだけ」かだけ。
      if (store !== undefined && store !== persistent) {
        await patchSection(store, section, patch);
      }
      await patchSection(persistent, section, patch);
    },

    start: async () => {
      const tab = await activeTab();
      if (tab === undefined) {
        // 相手が分からないときは永続の保存値をそのまま見せる。組み込みの既定値を見せると、
        // 保存してある設定が消えたように見える。
        store = persistent;
        for (const follower of followers) attach(follower, generation);
        return;
      }
      windowId = tab.windowId;
      // 切り替えの購読を相手を決めるより先に張る。決めている最中の切り替えを取りこぼさないため。
      onActivated((activated) => {
        if (activated.windowId !== windowId) return;
        void retarget(activated.tabId);
      });
      await retarget(tab.tabId);
    },
  };
};

/** 実ブラウザでの繋ぎ先。 */
const browserOptions: TargetTabOptions = {
  activeTab: async () => {
    // サイドパネルはウィンドウに属するので、currentWindow でパネル自身のウィンドウが取れる。
    // タブ番号を得るだけなら tabs 権限は要らない（要るのは url などを読むとき）。
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    return tab?.id === undefined
      ? undefined
      : { tabId: tab.id, windowId: tab.windowId };
  },
  onActivated: (listener) => {
    chrome.tabs.onActivated.addListener(({ tabId, windowId }) => {
      listener({ tabId, windowId });
    });
  },
  storeOfTab: (tabId) => tabScopedStore(sessionSettingsStore, tabId),
  persistent: localSettingsStore,
  sections: PERSISTED_SECTIONS,
};

const targetTab = createTargetTab(browserOptions);

/** 相手のタブの値を描く。詳細は TargetTab.follow。 */
export const followTargetTab = targetTab.follow;

/** 相手のタブへ当てる。詳細は TargetTab.apply。 */
export const applyToTargetTab = targetTab.apply;

/** 相手のタブへフィールド単位で当てる。詳細は TargetTab.patch。 */
export const patchTargetTab = targetTab.patch;

/** 相手を探し始める。パネルの入口が、操作 UI をすべて読み込んだ後で 1 度だけ呼ぶ。 */
export const startTargetTab = targetTab.start;
