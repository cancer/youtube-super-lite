import {
  asUntrustedRecord,
  clampToRange,
  type NumericRange,
  type SettingsSection,
} from "./settings";

/**
 * 要件 R4 のイコライザの値の仕様。
 *
 * バンド構成は多バンドではなく 3 ノブ（ボイス帯域の peaking・ローパス・ハイパス）で、
 * 定数はネイティブ実装の `EqParams` から値ごと引き継いだもの。**拡張側の都合で変えてはならない。**
 *
 * 音を出す側（Web Audio）の語彙はこのモジュールに閉じない代わりに、外へ出すのは
 * dB と Hz とフィルタ種別だけにしてある。ノードの生成や接続は main/audio-graph が持つ。
 */

/** ボイス帯域 peaking の中心周波数。人声の明瞭度に効く 1〜3kHz の中心。 */
export const VOICE_FREQ_HZ = 1800;

/** ボイス帯域 peaking の Q。1.2 ≒ 1〜3kHz を緩やかに持ち上げる幅。 */
export const VOICE_Q = 1.2;

/** ボイス帯域ゲインの値域（±dB）。UI のスライダーの端もこの値を唯一の出所とする。 */
export const VOICE_GAIN_MAX_DB = 12;

/** ボイス帯域ゲインの範囲と既定値。既定の 0dB は「オフ」を表す。 */
export const VOICE_GAIN_DB: NumericRange = {
  min: -VOICE_GAIN_MAX_DB,
  max: VOICE_GAIN_MAX_DB,
  default: 0,
};

/** ローパスカットオフの段階。 */
export const LOWPASS_STEPS: readonly number[] = [
  1000, 1500, 2000, 3000, 4000, 6000, 8000, 12000, 16000,
];

/** ハイパスカットオフの段階。 */
export const HIGHPASS_STEPS: readonly number[] = [
  40, 60, 80, 100, 150, 200, 300, 500, 1000,
];

/**
 * オフのハイパスを置く周波数。
 *
 * フィルタ段は常に 3 つ張ったままにして、オフはノードの取り外しではなく可聴域の外への
 * 退避で表す。設定を変えるたびにグラフを組み替えると、繋ぎ替えの瞬間に音が途切れるため。
 */
export const BYPASS_HIGHPASS_HZ = 10;

/** オフのローパスを置く周波数。理由は BYPASS_HIGHPASS_HZ と同じ。 */
export const BYPASS_LOWPASS_HZ = 24000;

/**
 * ローパス・ハイパスの Q。
 *
 * 移植元は Q を指定せず ffmpeg の lowpass / highpass の既定（0.707 ≒ 1/√2、
 * 通過域が最も平坦になる値）に委ねていた。Web Audio の BiquadFilterNode の既定は 1 で
 * カットオフ付近に山ができてしまうため、移植元の音を保つには明示的に置き直す必要がある。
 */
export const CUTOFF_Q = Math.SQRT1_2;

/**
 * イコライザ設定。全フィールド既定＝ニュートラル（フィルタ無し）。
 *
 * カットオフのオフを undefined ではなく null で表すのは、設定が chrome.storage を
 * 往復するため。undefined のフィールドは保存で消え、オフと「未保存」が区別できなくなる。
 */
export type EqualizerSettings = {
  /** ボイス帯域 peaking のゲイン dB。0 = オフ。 */
  readonly voiceGainDb: number;
  /** ローパスカットオフ Hz。null = オフ。 */
  readonly lowpassHz: number | null;
  /** ハイパスカットオフ Hz。null = オフ。 */
  readonly highpassHz: number | null;
};

/**
 * カットオフをラダーの両端へ収める。数値でない値はオフとして扱う。
 *
 * 段への量子化はしない（移植元の `clamped()` と同じ）。段の間の値でも音は出せるので、
 * 手編集や別経路で入った値を勝手に動かす理由がない。UI での表示は nearestStep が担う。
 */
const clampCutoff = (steps: readonly number[], value: unknown): number | null =>
  typeof value === "number" && Number.isFinite(value)
    ? Math.min(steps[steps.length - 1], Math.max(steps[0], value))
    : null;

/**
 * イコライザ設定の区画。
 *
 * normalize は移植元の `clamped()` にあたる。storage の手編集や旧版の残骸で範囲外の値が
 * 入り得るので、範囲は「保存時に守る規約」ではなく「読み出しで必ず通す関門」として扱う。
 */
export const equalizerSection: SettingsSection<EqualizerSettings> = {
  key: "equalizer",
  defaults: { voiceGainDb: 0, lowpassHz: null, highpassHz: null },
  normalize: (stored) => {
    const raw = asUntrustedRecord(stored);
    return {
      voiceGainDb: clampToRange(VOICE_GAIN_DB, raw.voiceGainDb),
      lowpassHz: clampCutoff(LOWPASS_STEPS, raw.lowpassHz),
      highpassHz: clampCutoff(HIGHPASS_STEPS, raw.highpassHz),
    };
  },
};

/** 3 ノブとも切れている（音に触らない）状態か。 */
export const isNeutral = (settings: EqualizerSettings): boolean =>
  settings.voiceGainDb === VOICE_GAIN_DB.default &&
  settings.lowpassHz === null &&
  settings.highpassHz === null;

/** フィルタ 1 段の値。Web Audio の BiquadFilterNode へそのまま写せる形にしてある。 */
export type FilterSpec = {
  readonly type: "highpass" | "peaking" | "lowpass";
  readonly frequencyHz: number;
  readonly q: number;
  readonly gainDb: number;
};

/**
 * 設定をフィルタ列へ変換する。並びがそのまま適用順序（highpass → peaking → lowpass）になる。
 *
 * オフのバンドも段としては残し、素通し相当の値を入れる（BYPASS_* を参照）。
 * 段数と並びが設定によらず一定なので、値の更新でグラフを組み替えずに済む。
 */
export const filterChainOf = (
  settings: EqualizerSettings,
): readonly FilterSpec[] => [
  {
    type: "highpass",
    frequencyHz: settings.highpassHz ?? BYPASS_HIGHPASS_HZ,
    q: CUTOFF_Q,
    gainDb: 0,
  },
  {
    type: "peaking",
    frequencyHz: VOICE_FREQ_HZ,
    q: VOICE_Q,
    gainDb: settings.voiceGainDb,
  },
  {
    type: "lowpass",
    frequencyHz: settings.lowpassHz ?? BYPASS_LOWPASS_HZ,
    q: CUTOFF_Q,
    gainDb: 0,
  },
];

/**
 * 値に最も近いラダーの段。
 *
 * 保存値は段の上にあるとは限らない（normalize は端へ寄せるだけ）ので、段を選ばせる UI は
 * これを通して表示位置を決める。設定そのものは書き換えない。
 */
export const nearestStep = (steps: readonly number[], value: number): number =>
  steps.reduce((best, step) =>
    Math.abs(value - step) < Math.abs(value - best) ? step : best,
  );
