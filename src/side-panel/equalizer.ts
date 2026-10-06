import {
  EQUALIZER_PRESETS,
  equalizerSection,
  isPreset,
  type EqualizerPreset,
  type EqualizerSettings,
} from "../shared/equalizer";
import { applyToTargetTab, followTargetTab } from "./target-tab";

/**
 * イコライザの操作 UI（要件 R4）。プリセットを 1 つ選ぶラジオの列。
 *
 * 周波数やゲインを個別に触らせない。利用者が選びたいのは用途（聞き取りたい・音楽を聴きたい）で、
 * 数値の中身は shared/equalizer が持つ。ここはプリセット名を選ばせて渡すだけで、当てる相手
 * （今見ているタブ）も保存先も配送経路も知らない（それらは target-tab が持つ）。
 */

/**
 * ラジオを集め、value がプリセット一覧と過不足なく対応していることを確かめる。
 *
 * HTML はこの拡張のものなので、対応が崩れていれば直すべき不具合。黙って一部だけ動かすと
 * 「選べないプリセットがある」「選んでも何も起きない」が実機で初めて見つかる。握り潰さず落とす。
 */
const mismatch = (): Error =>
  new Error("side-panel.html の eq-preset が shared/equalizer のプリセットと対応していない");

const collectPresetRadios = (): ReadonlyMap<EqualizerPreset, HTMLInputElement> => {
  const radios = new Map<EqualizerPreset, HTMLInputElement>();
  for (const input of document.querySelectorAll<HTMLInputElement>(
    'input[name="eq-preset"]',
  )) {
    // 知らない value と重複は、ここで見つけた時点で落とす。
    if (!isPreset(input.value) || radios.has(input.value)) throw mismatch();
    radios.set(input.value, input);
  }
  // 残るずれは不足だけ。value が全部一覧にあり重複も無いので、数が合えば過不足なし。
  if (radios.size !== EQUALIZER_PRESETS.length) throw mismatch();
  return radios;
};

const presetRadios = collectPresetRadios();

const render = (settings: EqualizerSettings): void => {
  for (const [preset, radio] of presetRadios) {
    radio.checked = preset === settings.preset;
  }
};

// 区画を持つのはこのパネルだけなので、フィールド単位の patch ではなく区画ごと当てる。
for (const [preset, radio] of presetRadios) {
  radio.addEventListener("change", () => {
    void applyToTargetTab(equalizerSection, { preset });
  });
}

followTargetTab(equalizerSection, render);
