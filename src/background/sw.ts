import { PERSISTED_SECTIONS } from "../shared/sections";
import { localSettingsStore, repairSection } from "../shared/settings";
import { isTabIdRequest } from "../shared/tab-id";
import { keysOfTab } from "../shared/tab-store";

/**
 * service worker。設定の集約点。
 *
 * 設定の値そのものはここを通らない。各コンテキストは storage を直接読み書きし、変更通知も
 * watchSection（storage.onChanged）が直接受け取る（理由は shared/settings の watchSection にある）。
 * ここが担うのは、storage だけでは足りない 3 つ。
 * - タブ単位の保存先を使うために要る「自分はどのタブか」への回答
 * - その保存先を content script から読めるようにする公開範囲の設定
 * - 閉じたタブぶんの後始末
 */

/**
 * ツールバーのアイコンのクリックでサイドパネルを開く。
 *
 * manifest の side_panel はパネルの中身を決めるだけで、開く操作は結びつけない。
 * サイドパネルはユーザー操作を起点にしか開けないため、既にある操作点であるアイコンへ結ぶ。
 * この設定が効く前提としてアイコンの宣言が要るので、manifest の action は中身が空でも消せない。
 * https://developer.chrome.com/docs/extensions/reference/api/sidePanel
 *
 * onInstalled ではなく service worker の起動ごとに設定する。この設定はインストール時の
 * 1 回で永続するとは限らず、取りこぼすとアイコンが無反応になるため。冪等なので繰り返してよい。
 */
chrome.sidePanel
  .setPanelBehavior({ openPanelOnActionClick: true })
  .catch((error: unknown) => {
    console.error(error);
  });

/**
 * session を content script からも読めるようにする。
 *
 * 既定では session は拡張の特権のあるページ（service worker・サイドパネル）からしか見えない。
 * タブが今使っている設定はここに入り、当てるのは content script なので、公開範囲を広げないと
 * 設定が届かない。広げても届く先は ISOLATED world までで、ページ（YouTube）の JS からは見えない。
 * https://developer.chrome.com/docs/extensions/reference/api/storage#storage_areas
 *
 * setPanelBehavior と同じく起動ごとに行なう。冪等で、取りこぼすと設定が届かなくなる。
 */
const sessionOpened = chrome.storage.session
  .setAccessLevel({ accessLevel: "TRUSTED_AND_UNTRUSTED_CONTEXTS" })
  .catch((error: unknown) => {
    console.error(error);
  });

/**
 * content script に自分のタブ番号を答える。
 *
 * 答えを待たせるのは公開範囲を広げ終えてから。先に答えると、受け取った content script が
 * まだ読めない session を読みに行って、そのタブだけ設定が当たらないまま残る。
 */
chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (!isTabIdRequest(message)) return false;
  // タブの無い送り主（サイドパネル・別の拡張ページ）にはタブ番号が無い。undefined を返して
  // 「分からない」を伝える。問い合わせ側はそれを受けて全タブ共通の保存先へ退く。
  const tabId = sender.tab?.id;
  void sessionOpened.then(() => {
    sendResponse(tabId);
  });
  // 応答が非同期であることを伝える。返さないと待たずに経路が閉じる。
  return true;
});

/**
 * 閉じたタブぶんの設定を捨てる。
 *
 * 残しておいても session はブラウザを閉じれば消えるが、タブ番号は使い回されるので、消さないと
 * 後から同じ番号になったタブが他人の設定を引き継いでしまう。
 */
chrome.tabs.onRemoved.addListener((tabId) => {
  void (async () => {
    // キーの一覧は保存先にしか無いので全件引く。持ち回る量はタブ数 × 区画数で、後始末の
    // 頻度（タブを閉じたとき）を考えれば数える価値のある大きさにはならない。
    const stored = await chrome.storage.session.get(null);
    const keys = keysOfTab(Object.keys(stored), tabId);
    if (keys.length === 0) return;
    await chrome.storage.session.remove([...keys]);
  })();
});

chrome.runtime.onInstalled.addListener(() => {
  // 保存値を正規化して書き戻す。読み出し側が毎回クランプするので正しさはここに依存しないが、
  // 手編集や旧版で入った範囲外の値を storage に残したままにしない。
  for (const section of PERSISTED_SECTIONS) {
    void repairSection(localSettingsStore, section);
  }
});
