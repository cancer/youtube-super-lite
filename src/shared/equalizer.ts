import { asUntrustedRecord, type SettingsSection } from "./settings";

/**
 * 要件 R4 のイコライザの値の仕様。
 *
 * 設定は周波数やゲインを個別に触る形ではなく、用途で選ぶプリセットにしてある。利用者が
 * 選びたいのは「何 Hz を何 dB」ではなく「聞き取りたい」「音楽を聴きたい」で、ノブを並べると
 * 調整の責任を利用者に押し付けることになるため。プリセットの中身（各段の値）はこのモジュールが
 * 唯一の出所で、UI は名前を選ばせるだけにする。
 *
 * - clarity … 聞き取り最優先。ゲーム配信の銃声・エンジン音など重低音を抑え、人の声の
 *   明瞭帯域を立てる。
 * - music … 音楽向け。低音の量感を少し足しつつ、300 Hz 付近の濁りを掘ってボーカルの
 *   輪郭を残す。
 *
 * 音を出す側（Web Audio）の語彙はこのモジュールに閉じない代わりに、外へ出すのは
 * dB と Hz とフィルタ種別だけにしてある。ノードの生成や接続は main/audio-graph が持つ。
 */

/**
 * プリセット名の一覧。型・UI の選択肢の検証・normalize の判定はすべてここから導く。
 * 一覧と型を別々に書くと、片方だけ足したときに型は通るのに実行時に落ちる。
 */
export const EQUALIZER_PRESETS = ["off", "clarity", "music"] as const;

export type EqualizerPreset = (typeof EQUALIZER_PRESETS)[number];

/** イコライザ設定。既定の off がニュートラル（フィルタ無し）。 */
export type EqualizerSettings = {
  readonly preset: EqualizerPreset;
};

/** 保存値や HTML の value のように外から来た値が、プリセット名のどれかであるか。 */
export const isPreset = (value: unknown): value is EqualizerPreset =>
  typeof value === "string" &&
  (EQUALIZER_PRESETS as readonly string[]).includes(value);

/** 音に触らない設定。既定値であり、読み出せない保存値の落とし先でもある。 */
const NEUTRAL: EqualizerSettings = { preset: "off" };

/**
 * イコライザ設定の区画。
 *
 * storage の手編集や旧版の残骸（3 ノブ時代の `{ voiceGainDb, lowpassHz, highpassHz }`）が
 * 入り得るので、プリセット名は「保存時に守る規約」ではなく「読み出しで必ず通す関門」として扱う。
 * 旧形式からの移行はしない。旧 3 ノブの値はどのプリセットにも対応しないので、推測で音を
 * 変えるより off で読み出して選び直してもらう方が利用者の意図に近い。
 *
 * 未保存でも必ず値を返す。shared/settings は normalize が undefined を返さないことを前提に
 * 「未保存」と「拡張コンテキストの失効」を区別している。
 */
export const equalizerSection: SettingsSection<EqualizerSettings> = {
  key: "equalizer",
  defaults: NEUTRAL,
  normalize: (stored) => {
    const raw = asUntrustedRecord(stored).preset;
    return { preset: isPreset(raw) ? raw : NEUTRAL.preset };
  },
};

/** 音に触らない状態か。 */
export const isNeutral = (settings: EqualizerSettings): boolean =>
  settings.preset === NEUTRAL.preset;

/**
 * ローパス・ハイパスの Q。
 *
 * Web Audio の lowpass / highpass は Q を dB として解釈する（α = sin(ω0) / (2·10^(Q/20))）。
 * 1/√2 ≒ 0.707 を渡すと通過域は完全には平坦にならず、カットオフ付近に約 +1.7 dB の山が残る
 * （平坦になるのは Q ≈ -3.01 dB）。要件 R4 はこの山を承知のうえで 1/√2 と定めているので、
 * その値をそのまま使う。値を変えるなら要件文書と BYPASS_FILTER の doc（同じ理由で素通しを
 * カットオフの退避で表さない）も合わせて見直す。
 */
const CUTOFF_Q = Math.SQRT1_2;

/**
 * フィルタ 1 段の値。Web Audio の BiquadFilterNode へそのまま写せる形にしてある。
 *
 * 4 つの値は段の種別によらず必ず埋める。Web Audio は lowshelf / highshelf で Q を、
 * lowpass / highpass で gain を無視するが、audio-graph の書き込みは種別を見ずに 4 値とも
 * 書くので、使わない値にも書ける数値が要る。
 */
export type FilterSpec = {
  readonly type: "highpass" | "lowshelf" | "peaking" | "highshelf" | "lowpass";
  readonly frequencyHz: number;
  readonly q: number;
  readonly gainDb: number;
};

/**
 * 素通しの段。
 *
 * フィルタ段は常に張ったままにして、オフはノードの取り外しではなく値で表す。設定を変える
 * たびにグラフを組み替えると、繋ぎ替えの瞬間に音が途切れるため。
 *
 * peaking のゲイン 0 dB は周波数・Q によらず完全に平坦になる。旧方式の「カットオフを
 * 可聴域の外へ退避」は見かけ上オフでも平坦にならない（10 Hz の highpass は Q=1/√2 でも
 * 通過域に約 +1.7 dB の山が残る）ので、素通しとしてはこちらが確実。
 */
const BYPASS_FILTER: FilterSpec = {
  type: "peaking",
  frequencyHz: 1000,
  q: 1,
  gainDb: 0,
};

/**
 * プリセットごとのフィルタ列。段数は 4 で固定し、並びがそのまま適用順序になる。
 *
 * 段の種別はプリセットごとに変わる（clarity はカットオフで挟む、music は shelf で挟む）。
 * 種別の違いは値の 1 つとしてノードに書くので、グラフの組み替えは起きない。
 */
const PRESET_CHAINS: Record<EqualizerPreset, readonly FilterSpec[]> = {
  off: [BYPASS_FILTER, BYPASS_FILTER, BYPASS_FILTER, BYPASS_FILTER],
  clarity: [
    // 200 Hz 以下（銃声・エンジン音・足音の重低音）を切る。
    { type: "highpass", frequencyHz: 200, q: CUTOFF_Q, gainDb: 0 },
    // 残った低域の量感も落として声の土台を軽くする。
    { type: "lowshelf", frequencyHz: 300, q: 1, gainDb: -6 },
    // 子音の明瞭度が決まる 3 kHz を立てる。
    { type: "peaking", frequencyHz: 3000, q: 1.0, gainDb: 4 },
    // 8 kHz より上のシャリつき・環境音を落とす。
    { type: "lowpass", frequencyHz: 8000, q: CUTOFF_Q, gainDb: 0 },
  ],
  music: [
    { type: "lowshelf", frequencyHz: 100, q: 1, gainDb: 3 },
    // 300 Hz 付近は低音の量感を足すと一緒に膨らんで濁る帯域。ここだけ掘ってボーカルの輪郭を残す。
    { type: "peaking", frequencyHz: 300, q: 1.2, gainDb: -2 },
    { type: "peaking", frequencyHz: 2500, q: 1.0, gainDb: 1.5 },
    { type: "highshelf", frequencyHz: 8000, q: 1, gainDb: 2 },
  ],
};

/**
 * 設定をフィルタ列へ変換する。
 *
 * 返る列は設定によらず常に 4 段で、並びがそのまま適用順序。段数と並びが一定なので、
 * 設定の更新でグラフを組み替えずに値の書き換えだけで済む（audio-graph の不変条件）。
 */
export const filterChainOf = (
  settings: EqualizerSettings,
): readonly FilterSpec[] => PRESET_CHAINS[settings.preset];
