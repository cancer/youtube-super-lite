import { describe, expect, test } from "bun:test";

import {
  BYPASS_HIGHPASS_HZ,
  BYPASS_LOWPASS_HZ,
  CUTOFF_Q,
  equalizerSection,
  filterChainOf,
  headroomGainDb,
  HIGHPASS_STEPS,
  isNeutral,
  LOWPASS_STEPS,
  nearestStep,
  VOICE_FREQ_HZ,
  VOICE_GAIN_MAX_DB,
  VOICE_Q,
  type EqualizerSettings,
} from "../src/shared/equalizer";
import {
  readSection,
  writeSection,
  type SettingsStore,
  type StoredChange,
} from "../src/shared/settings";

/**
 * ネイティブ実装の `EqParams` から引き継いだ値の仕様を固定する。
 *
 * バンド構成（1800Hz / Q1.2 / ±12dB、カットオフの段階）は移植元が唯一の出所であり、
 * 拡張側の都合で変えてはならない。数値をテストに直書きしてあるのは、定数を書き換えたときに
 * 定数を参照するアサーションが一緒にずれて素通りするのを防ぐため。
 */

const fakeStore = (
  stored: Record<string, unknown> = {},
): { store: SettingsStore; stored: Record<string, unknown> } => {
  const listeners = new Set<(changes: Record<string, StoredChange>) => void>();
  const store: SettingsStore = {
    get: async (keys) =>
      Object.fromEntries(
        keys.filter((key) => key in stored).map((key) => [key, stored[key]]),
      ),
    set: async (items) => {
      Object.assign(stored, items);
      const changes = Object.fromEntries(
        Object.entries(items).map(([key, value]) => [key, { newValue: value }]),
      );
      for (const listener of listeners) listener(changes);
    },
    onChanged: {
      addListener: (listener) => {
        listeners.add(listener);
      },
      removeListener: (listener) => {
        listeners.delete(listener);
      },
    },
    // 失効はこのフェイクの関心の外。失効の扱いは shared/settings のテストが見る。
    isAlive: () => true,
  };
  return { store, stored };
};

const settings = (
  overrides: Partial<EqualizerSettings> = {},
): EqualizerSettings => ({ ...equalizerSection.defaults, ...overrides });

describe("移植元から引き継ぐ値", () => {
  test("ボイス帯域は 1800Hz / Q1.2 / ±12dB", () => {
    expect(VOICE_FREQ_HZ).toBe(1800);
    expect(VOICE_Q).toBe(1.2);
    expect(VOICE_GAIN_MAX_DB).toBe(12);
  });

  test("ローパスの段階", () => {
    expect(LOWPASS_STEPS).toEqual([
      1000, 1500, 2000, 3000, 4000, 6000, 8000, 12000, 16000,
    ]);
  });

  test("ハイパスの段階", () => {
    expect(HIGHPASS_STEPS).toEqual([40, 60, 80, 100, 150, 200, 300, 500, 1000]);
  });
});

describe("equalizerSection.normalize", () => {
  test("未保存はニュートラル（ゲイン 0・両カットオフともオフ）", () => {
    expect(equalizerSection.normalize(undefined)).toEqual({
      voiceGainDb: 0,
      lowpassHz: null,
      highpassHz: null,
    });
  });

  test("ゲインの上限超過を +12dB へ収める", () => {
    expect(equalizerSection.normalize({ voiceGainDb: 40 }).voiceGainDb).toBe(12);
  });

  test("ゲインの下限未満を -12dB へ収める", () => {
    expect(equalizerSection.normalize({ voiceGainDb: -40 }).voiceGainDb).toBe(
      -12,
    );
  });

  test("両端ちょうどのゲインはそのまま通す", () => {
    expect(equalizerSection.normalize({ voiceGainDb: 12 }).voiceGainDb).toBe(12);
    expect(equalizerSection.normalize({ voiceGainDb: -12 }).voiceGainDb).toBe(
      -12,
    );
  });

  test("数値でないゲインはニュートラル（0dB）へ落とす", () => {
    expect(equalizerSection.normalize({ voiceGainDb: "6" }).voiceGainDb).toBe(0);
  });

  test("ローパスをラダーの両端へ収める", () => {
    expect(equalizerSection.normalize({ lowpassHz: 100 }).lowpassHz).toBe(1000);
    expect(equalizerSection.normalize({ lowpassHz: 48000 }).lowpassHz).toBe(
      16000,
    );
  });

  test("ハイパスをラダーの両端へ収める", () => {
    expect(equalizerSection.normalize({ highpassHz: 1 }).highpassHz).toBe(40);
    expect(equalizerSection.normalize({ highpassHz: 5000 }).highpassHz).toBe(
      1000,
    );
  });

  test("ラダーの段でないカットオフは端に収まるかぎりそのまま通す", () => {
    // 移植元の clamped() は端へのクランプだけで段への量子化はしない。
    expect(equalizerSection.normalize({ lowpassHz: 5000 }).lowpassHz).toBe(5000);
  });

  test("カットオフの null はオフのまま", () => {
    expect(
      equalizerSection.normalize({ lowpassHz: null, highpassHz: null }),
    ).toEqual({ voiceGainDb: 0, lowpassHz: null, highpassHz: null });
  });

  test("数値でないカットオフはオフへ落とす", () => {
    expect(equalizerSection.normalize({ lowpassHz: "8000" }).lowpassHz).toBeNull();
  });

  test("設定として壊れた保存値でもニュートラルで読み出せる", () => {
    expect(equalizerSection.normalize("壊れた値")).toEqual({
      voiceGainDb: 0,
      lowpassHz: null,
      highpassHz: null,
    });
  });
});

describe("isNeutral", () => {
  test("既定値はニュートラル", () => {
    expect(isNeutral(equalizerSection.defaults)).toBe(true);
  });

  test("ゲイン 0.0 はオフ", () => {
    expect(isNeutral(settings({ voiceGainDb: 0 }))).toBe(true);
  });

  test("ゲインが 0 以外ならニュートラルでない", () => {
    expect(isNeutral(settings({ voiceGainDb: -0.5 }))).toBe(false);
  });

  test("ローパスが入っているとニュートラルでない", () => {
    expect(isNeutral(settings({ lowpassHz: 8000 }))).toBe(false);
  });

  test("ハイパスが入っているとニュートラルでない", () => {
    expect(isNeutral(settings({ highpassHz: 100 }))).toBe(false);
  });
});

describe("filterChainOf", () => {
  test("適用順序は highpass → peaking → lowpass", () => {
    expect(filterChainOf(equalizerSection.defaults).map((f) => f.type)).toEqual([
      "highpass",
      "peaking",
      "lowpass",
    ]);
  });

  test("ニュートラルでも 3 段を素通し相当の値で並べる", () => {
    expect(filterChainOf(equalizerSection.defaults)).toEqual([
      { type: "highpass", frequencyHz: BYPASS_HIGHPASS_HZ, q: CUTOFF_Q, gainDb: 0 },
      { type: "peaking", frequencyHz: 1800, q: 1.2, gainDb: 0 },
      { type: "lowpass", frequencyHz: BYPASS_LOWPASS_HZ, q: CUTOFF_Q, gainDb: 0 },
    ]);
  });

  test("オフのハイパスは可聴域の下（10Hz）へ退ける", () => {
    expect(filterChainOf(settings({ highpassHz: null }))[0].frequencyHz).toBe(10);
  });

  test("オフのローパスは可聴域の上（24kHz）へ退ける", () => {
    expect(filterChainOf(settings({ lowpassHz: null }))[2].frequencyHz).toBe(
      24000,
    );
  });

  test("カットオフが入っていればその周波数を使う", () => {
    const chain = filterChainOf(settings({ highpassHz: 150, lowpassHz: 6000 }));

    expect(chain[0].frequencyHz).toBe(150);
    expect(chain[2].frequencyHz).toBe(6000);
  });

  test("ボイス帯域のゲインだけが peaking に載る", () => {
    expect(filterChainOf(settings({ voiceGainDb: 7.5 }))[1]).toEqual({
      type: "peaking",
      frequencyHz: 1800,
      q: 1.2,
      gainDb: 7.5,
    });
  });
});

describe("headroomGainDb", () => {
  test("ブーストしていなければ下げない", () => {
    expect(headroomGainDb(settings({ voiceGainDb: 0 }))).toBe(0);
  });

  test("ブーストした分だけ下げる", () => {
    expect(headroomGainDb(settings({ voiceGainDb: 12 }))).toBe(-12);
    expect(headroomGainDb(settings({ voiceGainDb: 6 }))).toBe(-6);
  });

  test("カットは下げる理由が無いので 0dB", () => {
    expect(headroomGainDb(settings({ voiceGainDb: -6 }))).toBe(0);
  });

  test("カットオフだけ効いていても下げない", () => {
    expect(headroomGainDb(settings({ highpassHz: 150, lowpassHz: 6000 }))).toBe(
      0,
    );
  });

  /**
   * 効き目はこの打ち消しの関係そのものなので、値の組ではなく関係として固定する。
   *
   * 打ち消せるのは peaking が指定どおり持ち上げる分だけ。カットオフを使うと、Web Audio の
   * lowpass / highpass が Q を dB として解釈することによる通過域のレゾナンス（1 段あたり
   * 約 +1.74dB）が残るので、チェーン全体の最大値まで相殺できるわけではない。
   */
  test("peaking の上げ幅をちょうど打ち消す", () => {
    const boosted = settings({ voiceGainDb: 9 });

    expect(filterChainOf(boosted)[1].gainDb + headroomGainDb(boosted)).toBe(0);
  });
});

describe("nearestStep", () => {
  test("段の値はその段のまま", () => {
    expect(nearestStep(LOWPASS_STEPS, 6000)).toBe(6000);
  });

  test("段の間の値を最も近い段へ寄せる", () => {
    expect(nearestStep(LOWPASS_STEPS, 5000)).toBe(4000);
    expect(nearestStep(HIGHPASS_STEPS, 120)).toBe(100);
  });

  test("ラダーの外の値を最寄りの端へ寄せる", () => {
    expect(nearestStep(LOWPASS_STEPS, 100)).toBe(1000);
    expect(nearestStep(HIGHPASS_STEPS, 9000)).toBe(1000);
  });
});

describe("設定の永続化", () => {
  test("保存した設定をそのまま読み戻せる", async () => {
    const { store } = fakeStore();
    const value = settings({
      voiceGainDb: 6,
      lowpassHz: 8000,
      highpassHz: 100,
    });
    await writeSection(store, equalizerSection, value);

    expect(await readSection(store, equalizerSection)).toEqual(value);
  });

  test("オフのカットオフは保存を往復してもオフのまま", async () => {
    const { store, stored } = fakeStore();
    await writeSection(store, equalizerSection, settings({ voiceGainDb: 3 }));

    // JSON 化で消えない形で保存されていること（undefined ではなく null）を保存値側でも押さえる。
    expect(JSON.parse(JSON.stringify(stored.equalizer))).toEqual({
      voiceGainDb: 3,
      lowpassHz: null,
      highpassHz: null,
    });
    expect(await readSection(store, equalizerSection)).toEqual(
      settings({ voiceGainDb: 3 }),
    );
  });

  test("手編集で範囲外になった保存値は読み出しで収める", async () => {
    const { store } = fakeStore({
      equalizer: { voiceGainDb: 99, lowpassHz: 48000, highpassHz: 1 },
    });

    expect(await readSection(store, equalizerSection)).toEqual({
      voiceGainDb: 12,
      lowpassHz: 16000,
      highpassHz: 40,
    });
  });
});
