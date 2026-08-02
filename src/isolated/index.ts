import { PERSISTED_SECTIONS } from "../shared/sections";
import {
  localSettingsStore,
  seedSection,
  sessionSettingsStore,
  type SettingsStore,
} from "../shared/settings";
import { surfaceOf } from "../shared/surface";
import { requestTabId } from "../shared/tab-id";
import { tabScopedStore } from "../shared/tab-store";
import { startChatDisplay } from "./chat-display";
import { startChatResize } from "./chat-resize";
import { CHAT_ITEM_LIST_SELECTOR, startChatTrim } from "./chat-trim";
import { startSettingsDelivery } from "./deliver";
import { installWatchDeclutter } from "./watch-declutter";

/**
 * ISOLATED world の入口。
 *
 * 拡張 API（chrome.storage）に触れられるのは MAIN world ではなくこちらなので、設定の読み出しと
 * MAIN world への配送、および DOM を対象にする機能がここに集まる。
 *
 * 何をどう変えるかも、いつ変えるかも、それぞれの機能のモジュールが持つ。この入口は
 * 「どの面でどれを起動するか」と「どの保存先から設定を読ませるか」だけを書く場所で、
 * 判断を足さない。
 */

/**
 * このタブの設定の保存先。
 *
 * サイドパネルの操作は「今見ているタブ」だけに効く。そのためにタブごとの保存先を使い、
 * まだ何も入っていなければ永続の保存値（＝新しいタブが最初に使う値）で埋めてから渡す。
 *
 * タブ番号が分からないときは全タブ共通の保存先で動く。設定が効かなくなるよりは、
 * タブごとに分かれない状態で効いたほうがましなため。
 */
const settingsStoreOfThisTab = async (): Promise<SettingsStore> => {
  const tabId = await requestTabId();
  if (tabId === undefined) return localSettingsStore;
  const store = tabScopedStore(sessionSettingsStore, tabId);
  await Promise.all(
    PERSISTED_SECTIONS.map((section) =>
      seedSection(localSettingsStore, store, section),
    ),
  );
  return store;
};

// 面の判定は保存先を待つ前に済ませる。待っている間に SPA 遷移が起きると location が動くため。
const surface = surfaceOf(location.pathname);

// チャット項目の上限を当て続ける（R3 の DOM 層）。watch ページにも注入されるが、
// そちらではセレクタが一致せず何も起きない。設定を持たないので保存先を待たない。
startChatTrim(() => document.querySelector(CHAT_ITEM_LIST_SELECTOR));

// 設定を使う機能は、このタブの保存先が決まってから始める。決まるまでの待ちは service worker への
// 問い合わせ 1 往復で、いずれの機能ももともと保存値の読み出しを待って動き出す。
void settingsStoreOfThisTab().then((store) => {
  // 設定を MAIN world（R4 のイコライザなど）へ配る。
  startSettingsDelivery({ store });

  // watch ページから「次の動画」の列とコメント欄を消す（R2）。この content script は
  // ライブチャットの iframe（/live_chat）にも入るが、整理の対象は watch ページだけ。
  if (surface === "watch") {
    installWatchDeclutter({ store });
    // チャットの幅を掴んで変えるハンドル（R5）。置き先は watch ページの列で、
    // 保存は「このタブ」と「次に開くタブの初期値」の両方に要るので保存先を 2 つ渡す。
    startChatResize({ store, persistent: localSettingsStore });
  }

  // チャットの文字サイズとパネル幅（R5）。DOM へ CSS を当てるので MAIN world へは配らず、
  // この world の各文書（watch とライブチャットの iframe）へ当てる。
  startChatDisplay({ store });
});
