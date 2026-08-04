// 機能ごとの操作 UI は自分のモジュールに閉じる。ここは読み込むだけ。
import "./equalizer";
import {
  CHAT_FONT_SIZE_PX,
  chatDisplaySection,
  clampToRange,
  watchDeclutterSection,
  type ChatDisplaySettings,
  type NumericRange,
  type WatchDeclutterSettings,
} from "../shared/settings";
import {
  applyToTargetTab,
  followTargetTab,
  patchTargetTab,
  startTargetTab,
} from "./target-tab";

/**
 * サイドパネルの操作面。
 *
 * 表示と操作の配線だけを持つ。値の当て先（今見ているタブ）と保存は target-tab が引き受け、
 * 当てた設定は storage.onChanged 経由で content script へ届くので、ここから直接配送しない。
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
const fontSizeValue = requireElement("chat-font-size-value");

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

// 刻みは範囲と違って保存値の仕様ではなく、つまみの操作粒度。文字サイズは 1px。
useRange(fontSizeInput, CHAT_FONT_SIZE_PX, 1);

const render = (settings: ChatDisplaySettings): void => {
  fontSizeInput.value = String(settings.fontSizePx);
  fontSizeValue.textContent = `${settings.fontSizePx}px`;
};

/**
 * つまみを動かすたびに保存する。
 *
 * 書き換えるのは文字サイズのフィールドだけにする。同じ区画にある幅を操作するのはページ内の
 * ハンドルなので、区画ごと書くとこちらの手元の値で相手の操作を打ち消してしまう。
 *
 * 表示は保存の往復を待たずにその場で更新する。待つと数値の表示がつまみから遅れて見えるうえ、
 * 操作中に前後した変更通知が届いたときに古い値へ戻って見える。
 */
const save = (): void => {
  const fontSizePx = clampToRange(CHAT_FONT_SIZE_PX, Number(fontSizeInput.value));
  fontSizeValue.textContent = `${fontSizePx}px`;
  void patchTargetTab(chatDisplaySection, { fontSizePx });
};

fontSizeInput.addEventListener("input", save);

followTargetTab(chatDisplaySection, render);

const removeCommentsInput = document.getElementById("remove-comments");
if (!(removeCommentsInput instanceof HTMLInputElement)) {
  throw new Error("side-panel.html に #remove-comments のチェックボックスが無い");
}

removeCommentsInput.addEventListener("change", () => {
  void applyToTargetTab(watchDeclutterSection, {
    removeComments: removeCommentsInput.checked,
  });
});

// 表示は必ず相手のタブの値から作る。タブを切り替えたときも、そのタブで別のパネルから
// 変えられたときも追従させるため、読み出しと変更購読の両方を同じ描画へ通す。
const renderWatchDeclutter = (settings: WatchDeclutterSettings): void => {
  removeCommentsInput.checked = settings.removeComments;
};

followTargetTab(watchDeclutterSection, renderWatchDeclutter);

// 操作 UI の登録がすべて済んでから相手を探す。探し当てた時点で、登録してあるぶんが一斉に描かれる。
void startTargetTab();
