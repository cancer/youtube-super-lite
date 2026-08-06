import { describe, expect, spyOn, test } from "bun:test";

import {
  amplitudeOf,
  connectChain,
  createEqualizer,
  createSourceCache,
  type GraphNode,
  type MediaChain,
} from "../src/main/audio-graph";
import { equalizerSection, type EqualizerSettings } from "../src/shared/equalizer";

/**
 * Web Audio のグラフの取り回しを、実ブラウザ無しで確かめられる部分に絞って固定する。
 *
 * 検査対象は「いつチェーンを作り、いつ作り直し、いつ古いものを捨てるか」であって、
 * BiquadFilterNode が実際に音を変えることではない（それは聴かないと確かめられない）。
 * そのため <video> は同一性の判定にしか使われない値として、チェーンはフェイクとして渡す。
 */

type FakeChain = MediaChain & {
  readonly media: object;
  readonly disconnected: () => boolean;
};

const chainSpy = (): {
  create: (media: object) => MediaChain;
  chains: FakeChain[];
} => {
  const chains: FakeChain[] = [];
  return {
    chains,
    create: (media) => {
      let released = false;
      const chain: FakeChain = {
        media,
        filters: [0, 1, 2].map(() => ({
          type: "peaking",
          frequency: { value: 0 },
          Q: { value: 0 },
          gain: { value: 0 },
        })),
        headroom: { gain: { value: 1 } },
        disconnect: () => {
          released = true;
        },
        disconnected: () => released,
      };
      chains.push(chain);
      return chain;
    },
  };
};

const active: EqualizerSettings = {
  ...equalizerSection.defaults,
  voiceGainDb: 6,
};

/**
 * 生成の失敗は運用中に原因を追えるよう警告として残る仕様なので、失敗経路のテストでは
 * 出力を伏せる。伏せずに走らせると、通っているテストの出力が警告で埋まる。
 */
const silenceWarnings = (): void => {
  spyOn(console, "warn").mockImplementation(() => {});
};

type FakeNode = GraphNode & { readonly disconnected: () => boolean };

/**
 * 繋ぎ先を記録するノード。`broken` にすると、そのノードから先へ繋ごうとした時点で失敗する
 * （＝チェーンの組み立てが途中で折れる状況を作る）。
 */
const graphSpy = (): {
  node: (broken?: boolean) => FakeNode;
  wired: GraphNode[];
} => {
  const wired: GraphNode[] = [];
  return {
    wired,
    node: (broken = false) => {
      let released = false;
      return {
        connect: (destination) => {
          if (broken) throw new Error("繋げない");
          wired.push(destination);
          return destination;
        },
        disconnect: () => {
          released = true;
        },
        disconnected: () => released,
      };
    },
  };
};

describe("connectChain", () => {
  test("source → 渡された段 → destination の順に繋ぐ", () => {
    const spy = graphSpy();
    const stages = [spy.node(), spy.node()];
    const destination = spy.node();

    connectChain(spy.node(), stages, destination);

    expect(spy.wired).toEqual([...stages, destination]);
  });

  test("段が 1 つも無ければ source を destination へ直結する", () => {
    const spy = graphSpy();
    const destination = spy.node();

    connectChain(spy.node(), [], destination);

    expect(spy.wired).toEqual([destination]);
  });

  /**
   * 音の出口を失わせないための最後の砦。source を作った時点で <video> の音はグラフ側へ
   * 移っており、繋ぎ終える前に諦めると無音になる。
   */
  test("組み立てが途中で折れても source を destination へ繋ぎ直す", () => {
    const spy = graphSpy();
    const source = spy.node();
    const destination = spy.node();

    expect(() => {
      connectChain(source, [spy.node(), spy.node(true)], destination);
    }).toThrow();

    expect(spy.wired.at(-1)).toBe(destination);
    expect(source.disconnected()).toBe(true);
  });

  test("組み立てが途中で折れたら例外を投げ直す", () => {
    const spy = graphSpy();

    expect(() => {
      connectChain(spy.node(), [spy.node(true)], spy.node());
    }).toThrow("繋げない");
  });
});

describe("amplitudeOf", () => {
  test("0dB は等倍", () => {
    expect(amplitudeOf(0)).toBe(1);
  });

  test("-6dB はおよそ半分の振幅", () => {
    expect(amplitudeOf(-6)).toBeCloseTo(0.501, 3);
  });

  test("-20dB は 1/10 の振幅", () => {
    expect(amplitudeOf(-20)).toBeCloseTo(0.1, 10);
  });
});

describe("createSourceCache", () => {
  test("同じ要素には source を 1 つしか作らない", () => {
    // 二度目の生成は例外になり、その時点で <video> は出口を失う（無音）。
    const created: object[] = [];
    const sourceOf = createSourceCache((media: object) => {
      created.push(media);
      return { of: media };
    });
    const media = {};

    expect(sourceOf(media)).toBe(sourceOf(media));
    expect(created).toHaveLength(1);
  });

  test("要素が違えば別の source を作る", () => {
    const sourceOf = createSourceCache((media: object) => ({ of: media }));

    expect(sourceOf({})).not.toBe(sourceOf({}));
  });
});

describe("createEqualizer", () => {
  test("ニュートラルな設定のあいだは <video> にグラフを張らない", () => {
    const spy = chainSpy();
    const equalizer = createEqualizer(spy.create);

    equalizer.attach({});

    expect(spy.chains).toHaveLength(0);
  });

  test("フィルタが有効になった時点でグラフを張る", () => {
    const spy = chainSpy();
    const equalizer = createEqualizer(spy.create);
    equalizer.attach({});

    equalizer.setSettings(active);

    expect(spy.chains).toHaveLength(1);
  });

  test("<video> を渡されるまでは設定が有効でもグラフを張らない", () => {
    const spy = chainSpy();
    const equalizer = createEqualizer(spy.create);

    equalizer.setSettings(active);

    expect(spy.chains).toHaveLength(0);
  });

  test("同じ <video> を渡し直してもチェーンを作り直さない", () => {
    const spy = chainSpy();
    const equalizer = createEqualizer(spy.create);
    const media = {};
    equalizer.setSettings(active);
    equalizer.attach(media);

    equalizer.attach(media);
    equalizer.attach(media);

    expect(spy.chains).toHaveLength(1);
    expect(spy.chains[0].disconnected()).toBe(false);
  });

  test("<video> が差し替わったらチェーンを作り直す", () => {
    const spy = chainSpy();
    const equalizer = createEqualizer(spy.create);
    const first = {};
    const second = {};
    equalizer.setSettings(active);
    equalizer.attach(first);

    equalizer.attach(second);

    expect(spy.chains.map((chain) => chain.media)).toEqual([first, second]);
  });

  test("差し替えのときに古いチェーンを明示的に外す", () => {
    const spy = chainSpy();
    const equalizer = createEqualizer(spy.create);
    equalizer.setSettings(active);
    equalizer.attach({});

    equalizer.attach({});

    expect(spy.chains[0].disconnected()).toBe(true);
    expect(spy.chains[1].disconnected()).toBe(false);
  });

  test("設定の更新はチェーンを作り直さない", () => {
    const spy = chainSpy();
    const equalizer = createEqualizer(spy.create);
    equalizer.setSettings(active);
    equalizer.attach({});

    equalizer.setSettings({ ...active, voiceGainDb: -3, lowpassHz: 8000 });

    expect(spy.chains).toHaveLength(1);
  });

  test("設定の値をフィルタへ highpass → peaking → lowpass の順に書く", () => {
    const spy = chainSpy();
    const equalizer = createEqualizer(spy.create);
    equalizer.setSettings({
      voiceGainDb: -3,
      lowpassHz: 8000,
      highpassHz: 150,
    });

    equalizer.attach({});

    expect(
      spy.chains[0].filters.map((filter) => [
        filter.type,
        filter.frequency.value,
        filter.gain.value,
      ]),
    ).toEqual([
      ["highpass", 150, 0],
      ["peaking", 1800, -3],
      ["lowpass", 8000, 0],
    ]);
  });

  test("設定を更新すると既存のフィルタの値が書き換わる", () => {
    const spy = chainSpy();
    const equalizer = createEqualizer(spy.create);
    equalizer.setSettings(active);
    equalizer.attach({});

    equalizer.setSettings({ ...active, highpassHz: 300 });

    expect(spy.chains[0].filters[0].frequency.value).toBe(300);
  });

  test("フィルタを全部オフに戻すと素通し相当の値が書かれる", () => {
    const spy = chainSpy();
    const equalizer = createEqualizer(spy.create);
    equalizer.setSettings(active);
    equalizer.attach({});

    equalizer.setSettings(equalizerSection.defaults);

    expect(
      spy.chains[0].filters.map((filter) => filter.frequency.value),
    ).toEqual([10, 1800, 24000]);
  });

  /**
   * ブーストした分は最終段で下げる。下げないと、ヘッドルームの無い音源で出力が振り切れて歪む。
   */
  test("ブーストした分だけ最終段で振幅を下げる", () => {
    const spy = chainSpy();
    const equalizer = createEqualizer(spy.create);
    equalizer.setSettings({ ...active, voiceGainDb: 12 });

    equalizer.attach({});

    expect(spy.chains[0].headroom.gain.value).toBeCloseTo(0.2512, 4);
  });

  test("ブーストしていなければ最終段は等倍のまま", () => {
    const spy = chainSpy();
    const equalizer = createEqualizer(spy.create);
    equalizer.setSettings({ ...active, voiceGainDb: 0, lowpassHz: 8000 });

    equalizer.attach({});

    expect(spy.chains[0].headroom.gain.value).toBe(1);
  });

  test("設定を更新すると最終段の振幅も追随する", () => {
    const spy = chainSpy();
    const equalizer = createEqualizer(spy.create);
    equalizer.setSettings(active);
    equalizer.attach({});

    equalizer.setSettings({ ...active, voiceGainDb: 12 });

    expect(spy.chains[0].headroom.gain.value).toBeCloseTo(0.2512, 4);
  });

  /**
   * 最終段は 0dB でも繋いだままにする。ブーストの有無で着脱すると、その瞬間に音が途切れる。
   */
  test("ブーストを戻しても繋ぎ替えずに等倍へ戻すだけ", () => {
    const spy = chainSpy();
    const equalizer = createEqualizer(spy.create);
    equalizer.setSettings({ ...active, voiceGainDb: 12 });
    equalizer.attach({});

    equalizer.setSettings({ ...active, voiceGainDb: 0, lowpassHz: 8000 });

    expect(spy.chains).toHaveLength(1);
    expect(spy.chains[0].headroom.gain.value).toBe(1);
  });

  /**
   * <video> は SPA 遷移で差し替わり、そのたびにチェーンを張り直す。生まれたての GainNode は
   * 等倍なので、差し替えの経路で値を書き忘れると遷移するたびに歪みが戻る。
   */
  test("差し替えた <video> のチェーンにも最終段の振幅を書く", () => {
    const spy = chainSpy();
    const equalizer = createEqualizer(spy.create);
    equalizer.setSettings({ ...active, voiceGainDb: 12 });
    equalizer.attach({});

    equalizer.attach({});

    expect(spy.chains[1].headroom.gain.value).toBeCloseTo(0.2512, 4);
  });

  test("グラフを作れなくても例外を投げない", () => {
    silenceWarnings();
    const equalizer = createEqualizer(() => {
      throw new Error("createMediaElementSource が使えない");
    });
    equalizer.setSettings(active);

    expect(() => {
      equalizer.attach({});
    }).not.toThrow();
  });

  test("グラフを作れなかった <video> には作り直しを繰り返さない", () => {
    // 例外の原因（同じ <video> に二度目の source を作れない等）は設定を変えても消えない。
    silenceWarnings();
    let attempts = 0;
    const equalizer = createEqualizer(() => {
      attempts += 1;
      throw new Error("createMediaElementSource が使えない");
    });
    equalizer.setSettings(active);
    equalizer.attach({});

    equalizer.setSettings({ ...active, voiceGainDb: 9 });

    expect(attempts).toBe(1);
  });

  test("グラフを作れなかった後でも <video> が変われば作り直しを試みる", () => {
    silenceWarnings();
    let attempts = 0;
    const equalizer = createEqualizer((): MediaChain => {
      attempts += 1;
      throw new Error("createMediaElementSource が使えない");
    });
    equalizer.setSettings(active);
    equalizer.attach({});

    equalizer.attach({});

    expect(attempts).toBe(2);
  });
});
