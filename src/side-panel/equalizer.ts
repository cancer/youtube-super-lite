import {
  equalizerSection,
  HIGHPASS_STEPS,
  LOWPASS_STEPS,
  nearestStep,
  VOICE_GAIN_DB,
  type EqualizerSettings,
} from "../shared/equalizer";
import {
  localSettingsStore,
  readSection,
  watchSection,
  writeSection,
} from "../shared/settings";

/**
 * イコライザの操作 UI（要件 R4 の 3 ノブ）。
 *
 * 保存するだけで、適用はしない。storage.onChanged が content script へ直接届くので、
 * ここから MAIN world へ配送する経路は持たない。
 */

/** カットオフのオフを表す option の value。数値の段と混ざらない文字列にしてある。 */
const OFF = "off";

const requireElement = <T extends Element>(
  id: string,
  constructor: new () => T,
): T => {
  const element = document.getElementById(id);
  if (!(element instanceof constructor)) {
    throw new Error(`side-panel.html の #${id} が ${constructor.name} でない`);
  }
  return element;
};

const gainInput = requireElement("eq-voice-gain", HTMLInputElement);
const gainOutput = requireElement("eq-voice-gain-value", HTMLOutputElement);
const lowpassSelect = requireElement("eq-lowpass", HTMLSelectElement);
const highpassSelect = requireElement("eq-highpass", HTMLSelectElement);

gainInput.min = String(VOICE_GAIN_DB.min);
gainInput.max = String(VOICE_GAIN_DB.max);

const fillSteps = (
  select: HTMLSelectElement,
  steps: readonly number[],
): void => {
  select.append(new Option("オフ", OFF));
  for (const hz of steps) select.append(new Option(`${hz} Hz`, String(hz)));
};

fillSteps(lowpassSelect, LOWPASS_STEPS);
fillSteps(highpassSelect, HIGHPASS_STEPS);

const gainLabel = (db: number): string => (db > 0 ? `+${db} dB` : `${db} dB`);

/**
 * 保存値を選択肢へ対応させる。
 *
 * 保存値は段の上にあるとは限らない（読み出しの関門はラダーの端へ寄せるだけ）ので、
 * 最寄りの段を選ぶ。以後の保存は表示どおりの段になる。
 */
const cutoffOption = (steps: readonly number[], hz: number | null): string =>
  hz === null ? OFF : String(nearestStep(steps, hz));

const cutoffOf = (select: HTMLSelectElement): number | null =>
  select.value === OFF ? null : Number(select.value);

const render = (settings: EqualizerSettings): void => {
  gainInput.value = String(settings.voiceGainDb);
  gainOutput.textContent = gainLabel(settings.voiceGainDb);
  lowpassSelect.value = cutoffOption(LOWPASS_STEPS, settings.lowpassHz);
  highpassSelect.value = cutoffOption(HIGHPASS_STEPS, settings.highpassHz);
};

const save = (): void => {
  void writeSection(localSettingsStore, equalizerSection, {
    voiceGainDb: Number(gainInput.value),
    lowpassHz: cutoffOf(lowpassSelect),
    highpassHz: cutoffOf(highpassSelect),
  });
};

// つまみを動かしている間は表示だけを追随させ、保存は操作が終わる change まで待つ。
// input のたびに保存すると、ドラッグ 1 回で storage への書き込みが数十回走る。
gainInput.addEventListener("input", () => {
  gainOutput.textContent = gainLabel(Number(gainInput.value));
});
gainInput.addEventListener("change", save);
lowpassSelect.addEventListener("change", save);
highpassSelect.addEventListener("change", save);

void readSection(localSettingsStore, equalizerSection).then(render);
watchSection(localSettingsStore, equalizerSection, render);
