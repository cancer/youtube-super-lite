import { describe, expect, test } from "bun:test";

import {
  EQUALIZER_PRESETS,
  equalizerSection,
  filterChainOf,
  isNeutral,
} from "../src/shared/equalizer";
import { readSection, writeSection } from "../src/shared/settings";

import { fakeStore } from "./support/settings-store";

/**
 * イコライザのプリセットとフィルタ列の仕様を固定する。
 *
 * 各段の数値をテストに直書きしてあるのは、定数を書き換えたときに定数を参照するアサーションが
 * 一緒にずれて素通りするのを防ぐため。プリセットの中身を変えるなら、このテストも一緒に変える。
 */

describe("EQUALIZER_PRESETS", () => {
  test("off / clarity / music の 3 つ", () => {
    expect(EQUALIZER_PRESETS).toEqual(["off", "clarity", "music"]);
  });
});

describe("equalizerSection.normalize", () => {
  test("未保存は off", () => {
    expect(equalizerSection.normalize(undefined)).toEqual({ preset: "off" });
  });

  test("設定として壊れた保存値でも off で読み出せる", () => {
    expect(equalizerSection.normalize("壊れた値")).toEqual({ preset: "off" });
  });

  test("知らないプリセット名は off へ落とす", () => {
    expect(equalizerSection.normalize({ preset: "loud" })).toEqual({
      preset: "off",
    });
  });

  test("文字列でないプリセットは off へ落とす", () => {
    expect(equalizerSection.normalize({ preset: 1 })).toEqual({ preset: "off" });
  });

  test("旧形式（3 ノブ）の保存値は移行せず off にする", () => {
    expect(
      equalizerSection.normalize({
        voiceGainDb: 6,
        lowpassHz: 8000,
        highpassHz: 100,
      }),
    ).toEqual({ preset: "off" });
  });

  test("既知のプリセット名はそのまま通す", () => {
    expect(equalizerSection.normalize({ preset: "clarity" })).toEqual({
      preset: "clarity",
    });
    expect(equalizerSection.normalize({ preset: "music" })).toEqual({
      preset: "music",
    });
  });
});

describe("isNeutral", () => {
  test("既定値はニュートラル", () => {
    expect(isNeutral(equalizerSection.defaults)).toBe(true);
  });

  test("off はニュートラル", () => {
    expect(isNeutral({ preset: "off" })).toBe(true);
  });

  test("clarity はニュートラルでない", () => {
    expect(isNeutral({ preset: "clarity" })).toBe(false);
  });

  test("music はニュートラルでない", () => {
    expect(isNeutral({ preset: "music" })).toBe(false);
  });
});

describe("filterChainOf", () => {
  test("どのプリセットも 4 段", () => {
    for (const preset of EQUALIZER_PRESETS) {
      expect(filterChainOf({ preset })).toHaveLength(4);
    }
  });

  test("off の 4 段はすべてゲイン 0 の peaking（素通し）", () => {
    for (const filter of filterChainOf({ preset: "off" })) {
      expect(filter.type).toBe("peaking");
      expect(filter.gainDb).toBe(0);
    }
  });

  test("clarity は低域を削って声の帯域を立てる 4 段", () => {
    expect(filterChainOf({ preset: "clarity" })).toEqual([
      { type: "highpass", frequencyHz: 200, q: Math.SQRT1_2, gainDb: 0 },
      { type: "lowshelf", frequencyHz: 300, q: 1, gainDb: -6 },
      { type: "peaking", frequencyHz: 3000, q: 1.0, gainDb: 4 },
      { type: "lowpass", frequencyHz: 8000, q: Math.SQRT1_2, gainDb: 0 },
    ]);
  });

  test("music は低音を足しつつ 300Hz の濁りを掘る 4 段", () => {
    expect(filterChainOf({ preset: "music" })).toEqual([
      { type: "lowshelf", frequencyHz: 100, q: 1, gainDb: 3 },
      { type: "peaking", frequencyHz: 300, q: 1.2, gainDb: -2 },
      { type: "peaking", frequencyHz: 2500, q: 1.0, gainDb: 1.5 },
      { type: "highshelf", frequencyHz: 8000, q: 1, gainDb: 2 },
    ]);
  });
});

describe("設定の永続化", () => {
  test.each([...EQUALIZER_PRESETS])(
    "%s を保存してそのまま読み戻せる",
    async (preset) => {
      const { store } = fakeStore();
      await writeSection(store, equalizerSection, { preset });

      expect(await readSection(store, equalizerSection)).toEqual({ preset });
    },
  );

  test("手編集で壊れた保存値は読み出しで off に収める", async () => {
    const { store } = fakeStore({ equalizer: { preset: "loud" } });

    expect(await readSection(store, equalizerSection)).toEqual({ preset: "off" });
  });
});
