/**
 * 数値設定の許容範囲と既定値。
 *
 * 範囲外・未保存・型違いのすべてをこの 3 値で決着させるので、設定値の仕様はここだけを見れば分かる。
 */
export type NumericRange = {
  readonly min: number;
  readonly max: number;
  readonly default: number;
};

/**
 * R5 のチャット文字サイズ（px）。
 *
 * 範囲と既定値はネイティブ実装から引き継いだ値で、拡張側の都合で変えてはならない
 * （要件 R5「現行のクランプ範囲と既定値を引き継ぐ」）。
 */
export const CHAT_FONT_SIZE_PX: NumericRange = { min: 10, max: 28, default: 16 };

/** R5 のチャットパネル幅比（ビューポート幅に対する比）。範囲・既定値の出所は CHAT_FONT_SIZE_PX と同じ。 */
export const CHAT_PANEL_WIDTH_RATIO: NumericRange = {
  min: 0.15,
  max: 0.6,
  default: 0.28,
};

/**
 * 値を範囲へ収める。数値でない値は既定値へ落とす。
 *
 * 設定は storage の手編集や旧版の残骸で範囲外になり得るため、範囲は「保存時に守る規約」ではなく
 * 「読み出しで必ず通す関門」として扱う。
 */
export const clampToRange = (range: NumericRange, value: unknown): number =>
  typeof value === "number" && Number.isFinite(value)
    ? Math.min(range.max, Math.max(range.min, value))
    : range.default;

/**
 * 型システムの外から来た値をフィールド単位で読める形にする。
 *
 * 出所は storage の保存値と橋渡しのメッセージの両方で、どちらも信用しないので区画の normalize と
 * メッセージの判定はこれを通してから個々のフィールドを見る。
 */
export const asUntrustedRecord = (value: unknown): Record<string, unknown> =>
  typeof value === "object" && value !== null
    ? (value as Record<string, unknown>)
    : {};

/**
 * storage 上の設定 1 区画。
 *
 * 機能ごとに区画を独立させてあるので、別機能の設定（R4 の EQ など）は自分のモジュールで
 * この型の値を 1 つ定義するだけで同じ storage 基盤に載る。読み書き・購読・配送の各関数は
 * 区画の中身を知らない。
 */
export type SettingsSection<T> = {
  /** storage のキー。橋渡しのメッセージでも区画の識別子として使う。 */
  readonly key: string;
  /** 設定が届く前・保存が無い場合に使う値。素通しで動く状態を表す。 */
  readonly defaults: T;
  /** 型システムの外から来た値を正規化する。信頼境界の検証はこの 1 箇所に閉じる。 */
  readonly normalize: (stored: unknown) => T;
};

/** R5 のチャット表示設定。 */
export type ChatDisplaySettings = {
  readonly fontSizePx: number;
  readonly panelWidthRatio: number;
};

export const chatDisplaySection: SettingsSection<ChatDisplaySettings> = {
  key: "chatDisplay",
  defaults: {
    fontSizePx: CHAT_FONT_SIZE_PX.default,
    panelWidthRatio: CHAT_PANEL_WIDTH_RATIO.default,
  },
  normalize: (stored) => {
    const raw = asUntrustedRecord(stored);
    return {
      fontSizePx: clampToRange(CHAT_FONT_SIZE_PX, raw.fontSizePx),
      panelWidthRatio: clampToRange(CHAT_PANEL_WIDTH_RATIO, raw.panelWidthRatio),
    };
  },
};

/**
 * 真偽の設定を読み出す。真偽でない値は既定値へ落とす。
 *
 * 数値の clampToRange と同じ役割で、storage の手編集や旧版の残骸を読み出しの側で受け止める。
 */
export const toBoolean = (fallback: boolean, value: unknown): boolean =>
  typeof value === "boolean" ? value : fallback;

/** watch ページから消す部分の設定。 */
export type WatchDeclutterSettings = {
  /**
   * コメント欄を消すか。
   *
   * 「次の動画」の列と違って切り替えられるのは、ライブアーカイブのコメント欄に視聴位置の
   * タイムスタンプが投稿されるため。消えると困る動画があるので、消すかどうかは人が決める。
   */
  readonly removeComments: boolean;
};

/** 既定はコメント欄も消す。要る動画のときだけ人が戻す。 */
const watchDeclutterDefaults: WatchDeclutterSettings = { removeComments: true };

export const watchDeclutterSection: SettingsSection<WatchDeclutterSettings> = {
  key: "watchDeclutter",
  defaults: watchDeclutterDefaults,
  normalize: (stored) => ({
    removeComments: toBoolean(
      watchDeclutterDefaults.removeComments,
      asUntrustedRecord(stored).removeComments,
    ),
  }),
};

/** storage の変更 1 件。newValue が無い（キー削除）場合も normalize が既定値へ落とす。 */
export type StoredChange = {
  readonly newValue?: unknown;
};

/**
 * 設定が使う storage の操作だけを表した形。
 *
 * chrome.storage.local への依存を呼び出し側から渡す形にしてあるので、テストはフェイクを差せる。
 */
export type SettingsStore = {
  get(keys: string[]): Promise<Record<string, unknown>>;
  set(items: Record<string, unknown>): Promise<void>;
  onChanged: {
    addListener(listener: (changes: Record<string, StoredChange>) => void): void;
    removeListener(
      listener: (changes: Record<string, StoredChange>) => void,
    ): void;
  };
};

/**
 * 実際の保存先。sync ではなく local を使う（端末間同期は要件に無く、クォータ制約だけが増える）。
 *
 * chrome への参照を関数の中に閉じてあるのは、拡張 API が無い実行環境で本モジュールを
 * 読み込めるようにするため。
 */
export const localSettingsStore: SettingsStore = {
  get: (keys) => chrome.storage.local.get(keys),
  set: (items) => chrome.storage.local.set(items),
  onChanged: {
    addListener: (listener) => chrome.storage.local.onChanged.addListener(listener),
    removeListener: (listener) =>
      chrome.storage.local.onChanged.removeListener(listener),
  },
};

/** 区画を読む。保存値が範囲外・型違い・未保存のいずれでも、返る値は必ず正規化済み。 */
export const readSection = async <T>(
  store: SettingsStore,
  section: SettingsSection<T>,
): Promise<T> => section.normalize((await store.get([section.key]))[section.key]);

/** 区画を保存する。 */
export const writeSection = async <T>(
  store: SettingsStore,
  section: SettingsSection<T>,
  value: T,
): Promise<void> => store.set({ [section.key]: value });

/**
 * 区画の変更を購読する。
 *
 * storage.onChanged は拡張の全コンテキスト（service worker / content script / サイドパネル）へ届くため、
 * service worker による中継を挟まない。挟むと同じ変更が二重に流れる。
 */
export const watchSection = <T>(
  store: SettingsStore,
  section: SettingsSection<T>,
  onChange: (value: T) => void,
): (() => void) => {
  const listener = (changes: Record<string, StoredChange>): void => {
    if (!(section.key in changes)) return;
    onChange(section.normalize(changes[section.key].newValue));
  };
  store.onChanged.addListener(listener);
  return () => store.onChanged.removeListener(listener);
};

/**
 * 保存値を正規化して書き戻す。
 *
 * 読み出しは毎回クランプするので正しさはこれに依存しない。範囲外の値を storage に残したままに
 * しないための後始末であり、呼ぶのは設定の集約点（service worker）だけでよい。
 */
export const repairSection = async <T>(
  store: SettingsStore,
  section: SettingsSection<T>,
): Promise<T> => {
  const value = await readSection(store, section);
  await writeSection(store, section, value);
  return value;
};
