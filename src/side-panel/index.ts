import {
  chatDisplaySection,
  localSettingsStore,
  readSection,
  watchSection,
  type ChatDisplaySettings,
} from "../shared/settings";

/**
 * サイドパネルの骨。
 *
 * 設定の読み出しと変更購読の配線だけを持つ。操作 UI（R4 のノブ / R5 のスライダー）は
 * side-panel.html の対応する section へ差し込み、保存は shared/settings の writeSection を使う。
 * 保存すれば storage.onChanged 経由で content script へ届くので、ここから直接配送しない。
 */

const valuesElement = document.getElementById("chat-display-values");
if (valuesElement === null) {
  throw new Error("side-panel.html に #chat-display-values が無い");
}

/** 操作 UI が入るまでの仮表示。R5 の実装でスライダーの現在値表示に置き換わる。 */
const render = (settings: ChatDisplaySettings): void => {
  valuesElement.textContent = `文字サイズ ${settings.fontSizePx}px / パネル幅比 ${settings.panelWidthRatio}`;
};

void readSection(localSettingsStore, chatDisplaySection).then(render);
watchSection(localSettingsStore, chatDisplaySection, render);
