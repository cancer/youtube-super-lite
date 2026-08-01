import {
  CHAT_FONT_SIZE_PX,
  CHAT_PANEL_WIDTH_RATIO,
  chatDisplaySection,
  localSettingsStore,
  readSection,
  watchSection,
  writeSection,
  type ChatDisplaySettings,
  type NumericRange,
} from "../shared/settings";

/**
 * サイドパネルの操作面。
 *
 * 設定の読み出し・保存・変更購読の配線だけを持つ。保存すれば storage.onChanged 経由で
 * content script へ届くので、ここから直接配送しない。
 * 機能ごとの UI は side-panel.html の対応する section へ差し込む（R4 のイコライザは別）。
 */

const requireInput = (id: string): HTMLInputElement => {
  const element = document.getElementById(id);
  // HTML はこの拡張のものなので、対応が崩れていれば直すべき不具合。握り潰さず落とす。
  if (!(element instanceof HTMLInputElement)) {
    throw new Error(`side-panel.html に input#${id} が無い`);
  }
  return element;
};

const requireElement = (id: string): HTMLElement => {
  const element = document.getElementById(id);
  if (element === null) throw new Error(`side-panel.html に #${id} が無い`);
  return element;
};

const fontSizeInput = requireInput("chat-font-size");
const panelWidthInput = requireInput("chat-panel-width");
const fontSizeValue = requireElement("chat-font-size-value");
const panelWidthValue = requireElement("chat-panel-width-value");

/** スライダーの可動域を設定の範囲に合わせる。範囲の出所は shared/settings だけにする。 */
const useRange = (
  input: HTMLInputElement,
  range: NumericRange,
  step: number,
): void => {
  input.min = String(range.min);
  input.max = String(range.max);
  input.step = String(step);
};

// 刻みは範囲と違って保存値の仕様ではなく、つまみの操作粒度。文字サイズは 1px、比率は 1%。
useRange(fontSizeInput, CHAT_FONT_SIZE_PX, 1);
useRange(panelWidthInput, CHAT_PANEL_WIDTH_RATIO, 0.01);

const render = (settings: ChatDisplaySettings): void => {
  fontSizeInput.value = String(settings.fontSizePx);
  panelWidthInput.value = String(settings.panelWidthRatio);
  fontSizeValue.textContent = `${settings.fontSizePx}px`;
  // 比率はビューポート幅に対する割合なので、目で見て分かる百分率で出す。
  panelWidthValue.textContent = `${Math.round(settings.panelWidthRatio * 100)}%`;
};

/**
 * つまみを動かすたびに保存する。
 *
 * 表示は保存の往復を待たずにその場で更新する。待つと数値の表示がつまみから遅れて見えるうえ、
 * 操作中に前後した変更通知が届いたときに古い値へ戻って見える。
 */
const save = (): void => {
  const settings = chatDisplaySection.normalize({
    fontSizePx: Number(fontSizeInput.value),
    panelWidthRatio: Number(panelWidthInput.value),
  });
  render(settings);
  void writeSection(localSettingsStore, chatDisplaySection, settings);
};

for (const input of [fontSizeInput, panelWidthInput]) {
  input.addEventListener("input", save);
}

void readSection(localSettingsStore, chatDisplaySection).then(render);
watchSection(localSettingsStore, chatDisplaySection, render);
