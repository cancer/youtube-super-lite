import { subscribeSection } from "../shared/bridge";
import {
  equalizerSection,
  filterChainOf,
  headroomGainDb,
  isNeutral,
  type EqualizerSettings,
} from "../shared/equalizer";
import { onNavigated } from "../shared/navigation";

/**
 * 要件 R4 のイコライザを、ページの <video> に Web Audio のフィルタチェーンとして張る。
 *
 * MAIN world に置くのは <video> 要素そのものを掴む必要があるため。設定は拡張 API に触れる
 * ISOLATED world から shared/bridge 経由で届く。
 *
 * 守る不変条件は 3 つ。
 * 1. `createMediaElementSource` は <video> の音の出口をグラフ側へ移す。以後この拡張が
 *    音の経路の一部になるので、組み立ての失敗は無音として現れる。失敗は必ず握りつぶし、
 *    「イコライザが効かないだけ」に落とす。
 * 2. AudioContext はページ内で 1 つだけ。遷移のたびに作ると再生スレッドとバッファが積み上がり、
 *    このプロジェクトの最優先の評価軸（長時間視聴での定常メモリ増加）を自分で壊す。
 * 3. 設定の更新でグラフを組み替えない。段の構成はフィルタ 3 段 + ヘッドルームを空ける 1 段の
 *    固定構成で、変えるのは値だけ。
 */

/**
 * BiquadFilterNode のうち、イコライザが書き換える部分だけを表した形。
 *
 * type は実ノードと同じ広さ（BiquadFilterType）で持つ。実ノードを代入できる必要があるため
 * 使う 3 種類へは狭められない。値の側は FilterSpec が 3 種類に限っている。
 */
export type FilterNode = {
  type: BiquadFilterType;
  readonly frequency: { value: number };
  readonly Q: { value: number };
  readonly gain: { value: number };
};

/**
 * GainNode のうち、イコライザが書き換える部分だけを表した形。
 *
 * 型名を GainNode にしないのは、同名だと本物の GainNode（DOM lib のグローバル型）と読み分けが
 * つかなくなるため。宣言自体は shadow するだけでエラーにはならない。
 */
export type HeadroomNode = { readonly gain: { value: number } };

/** <video> 1 つに張ったチェーン（フィルタ 3 段 + ヘッドルームを空ける 1 段）。 */
export type MediaChain = {
  /** filterChainOf と同じ並び（highpass → peaking → lowpass）。 */
  readonly filters: readonly FilterNode[];
  /** ブースト分を下げてヘッドルームを空ける段。フィルタ列の後段に置く。 */
  readonly headroom: HeadroomNode;
  /** グラフから外す。<video> が差し替わったときに呼ぶ。 */
  readonly disconnect: () => void;
};

/**
 * <video> にチェーンを張る。張れない場合は例外を投げる。
 *
 * 例外を返り値ではなく throw で表すのは、Web Audio の API 自体が throw で失敗を伝えるため
 * （同じ要素に二度 source を作る、AudioContext を作れない、など）。
 */
export type ChainFactory<Media> = (media: Media) => MediaChain;

/** AudioNode のうち、繋ぎ替えに使う部分だけを表した形。 */
export type GraphNode = {
  connect(destination: GraphNode): GraphNode;
  disconnect(): void;
};

/**
 * source → 渡された段を並びのまま → destination と直列に繋ぐ。
 *
 * 途中で失敗したら、source を destination へ直結し直してから例外を投げ直す。source を
 * 作った時点で <video> の音は既にグラフ側へ移っており、繋ぎ終える前に諦めると出口の無い
 * source が残って無音になる。無音は利用者が原因を特定しにくい壊れ方なので、経路を保つ責任は
 * 「段を張れたか」とは独立に果たす。呼び出し側はイコライザを諦めるために例外を受け取る。
 */
export const connectChain = (
  source: GraphNode,
  stages: readonly GraphNode[],
  destination: GraphNode,
): void => {
  try {
    stages
      .reduce((upstream, stage) => upstream.connect(stage), source)
      .connect(destination);
  } catch (error) {
    // 途中まで繋がった枝は行き止まりなので、まとめて外してから繋ぎ直す。
    source.disconnect();
    source.connect(destination);
    throw error;
  }
};

/**
 * 要素ごとに source を 1 つだけ作る関数を組む。
 *
 * `createMediaElementSource` は同じ要素に二度呼べず、二度目は例外になる。一方で一度呼ぶと
 * その要素の音は恒久的にグラフ側へ移り、元の出力へは戻せない。作り直しのたびに呼ぶと、
 * 同じ <video> が戻ってきたときに例外で止まり、出口の無いまま無音になる。
 *
 * 覚え先が WeakMap なので、要素が捨てられれば source ごと回収される。
 */
export const createSourceCache = <Media extends object, Source>(
  create: (media: Media) => Source,
): ((media: Media) => Source) => {
  const sources = new WeakMap<Media, Source>();
  return (media) => {
    const reused = sources.get(media);
    if (reused !== undefined) return reused;
    const created = create(media);
    sources.set(media, created);
    return created;
  };
};

/**
 * dB を振幅比へ直す。
 *
 * 設定とフィルタの値は dB で扱うのに対し、GainNode.gain は線形の振幅比を取る。この差は
 * Web Audio のノード API の都合なので、換算は dB と Hz だけを外へ出す shared/equalizer では
 * なくノードを扱うこちら側に置く。
 */
export const amplitudeOf = (db: number): number => 10 ** (db / 20);

export type Equalizer<Media> = {
  /** 設定を差し替える。チェーンは作り直さず、ノードの値だけを書き換える。 */
  readonly setSettings: (settings: EqualizerSettings) => void;
  /** 現在の <video> を伝える。同じ要素なら何もしない。 */
  readonly attach: (media: Media) => void;
};

/**
 * イコライザの状態機械。Web Audio そのものは知らず、チェーンの生成だけを外から受け取る。
 *
 * グラフを張るのは設定がニュートラルでなくなってから。イコライザに触れていない利用者の音は
 * 最後まで Web Audio を経由せず、不変条件 1 のリスクを負わない。一度張ったチェーンは
 * ニュートラルへ戻しても残す（`createMediaElementSource` は取り消せず、素通し相当の値を
 * 入れておく方が繋ぎ替えより安全なため）。
 */
export const createEqualizer = <Media>(
  createChain: ChainFactory<Media>,
): Equalizer<Media> => {
  let settings = equalizerSection.defaults;
  /** 対象の <video>。attach を受けるまでは無い。 */
  let media: Media | undefined;
  let chain: MediaChain | undefined;
  /** 現在の <video> でチェーンの生成に失敗したか。失敗の原因は設定を変えても消えないので、再試行しない。 */
  let failed = false;

  const writeValues = (): void => {
    if (chain === undefined) return;
    const { filters, headroom } = chain;
    filterChainOf(settings).forEach((spec, index) => {
      const filter = filters[index];
      filter.type = spec.type;
      filter.frequency.value = spec.frequencyHz;
      filter.Q.value = spec.q;
      filter.gain.value = spec.gainDb;
    });
    headroom.gain.value = amplitudeOf(headroomGainDb(settings));
  };

  const ensureChain = (): void => {
    if (
      chain !== undefined ||
      failed ||
      media === undefined ||
      isNeutral(settings)
    ) {
      return;
    }
    try {
      chain = createChain(media);
    } catch (error) {
      // ここで諦めても音は鳴り続ける（グラフへ移す前か、生成側が出口を繋ぎ直した後）。
      failed = true;
      console.warn("[youtube-super-lite] イコライザを無効にしました", error);
    }
  };

  return {
    setSettings: (next) => {
      settings = next;
      ensureChain();
      writeValues();
    },
    attach: (next) => {
      if (next === media) return;
      chain?.disconnect();
      chain = undefined;
      failed = false;
      media = next;
      ensureChain();
      writeValues();
    },
  };
};

/**
 * ページで唯一の AudioContext を返す。
 *
 * 保持先を globalThis にするのは、content script が二重に注入されても 1 つに保つため
 * （main/intercept と同じ考え方で、寿命をページに合わせる）。
 */
const CONTEXT_KEY = "__youtubeSuperLiteAudioContext";

type ContextHolder = { [CONTEXT_KEY]?: AudioContext };

const sharedContext = (): AudioContext => {
  const holder = globalThis as ContextHolder;
  return (holder[CONTEXT_KEY] ??= new AudioContext());
};

const sourceOf = createSourceCache((media: HTMLMediaElement) =>
  sharedContext().createMediaElementSource(media),
);

const webAudioChain: ChainFactory<HTMLMediaElement> = (media) => {
  const context = sharedContext();
  // 自動再生ポリシーで suspended のまま繋ぐと音が止まる。resume は冪等なので毎回呼んでよい。
  if (context.state === "suspended") void context.resume();

  // ここを通った時点で <video> の音の出口はグラフ側へ移る。以降の失敗は無音を意味する。
  const source = sourceOf(media);
  // 段数はフィルタ列の長さに従う。種別と値は生成直後に呼び出し側（writeValues）が書く。
  const filters = filterChainOf(equalizerSection.defaults).map(() =>
    context.createBiquadFilter(),
  );
  /**
   * ヘッドルームを空ける段。ブーストが 0 のときも外さず、等倍で繋いだままにする（不変条件 3）。
   *
   * 最終段に置くのは規約であって数値上の必然ではない。線形フィルタとゲインは可換で、
   * 内部は float32 なので中間でクリップすることもない。順序を固定するのは、読み手が
   * 「出力直前の音量調整」として読めるようにするため。
   */
  const headroom = context.createGain();
  connectChain(source, [...filters, headroom], context.destination);
  return {
    filters,
    headroom,
    disconnect: () => {
      source.disconnect();
      for (const filter of filters) filter.disconnect();
      headroom.disconnect();
    },
  };
};

/**
 * 本編を再生しているプレーヤーの <video>。
 *
 * watch ページには関連動画のホバープレビューなど本編以外の <video> も現れる。単に最初の
 * <video> を採ると、プレビューが出入りするたびに対象が入れ替わり、その都度 source が増えて
 * 定常メモリを押し上げる。プレーヤー本体が付ける印で先に絞り、印が変わったときだけ
 * 最初の <video> へ落とす。
 */
const playerVideo = (): HTMLVideoElement | null =>
  document.querySelector("video.html5-main-video") ??
  document.querySelector("video");

/**
 * イコライザを組み込む。
 *
 * <video> は SPA 遷移で差し替わるので、遷移とメディアの読み込み開始の両方で見直す。
 * メディアイベントはバブルしないため、document で拾うにはキャプチャを使う。
 *
 * 見つからないときは何もしない（今のチェーンを外さない）。DOM の入れ替えの最中に一瞬
 * 見失うことがあり、そこで外すと繋ぎ直すまでの間だけ音が消えるため。
 */
export const installEqualizer = (): void => {
  const equalizer = createEqualizer(webAudioChain);
  subscribeSection(equalizerSection, (settings) => {
    equalizer.setSettings(settings);
  });

  const sync = (): void => {
    const media = playerVideo();
    if (media !== null) equalizer.attach(media);
  };
  onNavigated(sync);
  document.addEventListener("loadstart", sync, true);
};
