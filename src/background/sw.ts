import {
  chatDisplaySection,
  localSettingsStore,
  repairSection,
  watchDeclutterSection,
  type SettingsSection,
} from "../shared/settings";

/**
 * service worker。設定の集約点。
 *
 * 変更通知は watchSection（storage.onChanged）が各コンテキストへ直接届けるので、ここで中継しない
 * （理由は shared/settings の watchSection にある）。
 */

/**
 * storage に保存される全区画。設定を持つ機能（R4 など）はここへ自分の区画を足す。
 *
 * MAIN world へ配る区画（ISOLATED world 側の一覧）とは別物で、こちらは保存されているものを
 * すべて並べる。正規化の取りこぼしを作らないため。
 */
const persistedSections: readonly SettingsSection<unknown>[] = [
  chatDisplaySection,
  watchDeclutterSection,
];

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

chrome.runtime.onInstalled.addListener(() => {
  // 保存値を正規化して書き戻す。読み出し側が毎回クランプするので正しさはここに依存しないが、
  // 手編集や旧版で入った範囲外の値を storage に残したままにしない。
  for (const section of persistedSections) {
    void repairSection(localSettingsStore, section);
  }
});
