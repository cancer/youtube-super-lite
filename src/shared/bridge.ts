import { asUntrustedRecord, type SettingsSection } from "./settings";

/**
 * MAIN world ⇄ ISOLATED world の橋渡し。
 *
 * MAIN world の content script は拡張 API（chrome.storage）に触れない。一方で R4 の Web Audio は
 * ページの <video> を掴む必要があるので MAIN world にいる。よって設定は ISOLATED world が読み、
 * DOM の message 経路で MAIN world へ配る。
 *
 * この経路はページの JS からも観測できる。したがって流すのは「設定の配送」だけの一方向に限り、
 * 機微な情報（認証情報・ユーザー識別子など）は載せない。逆向きの要求経路も設けない。
 *
 * MAIN world が実際に chrome.storage へ触れるかどうかは未検証だが、触れる場合でもこの橋渡しは
 * 単に使われなくなるだけで、どちらでも成立する。
 */
const SETTINGS_MESSAGE_TYPE = "youtube-super-lite/settings";

/**
 * 配送メッセージ。
 *
 * type に拡張名の名前空間を付けてあるので、ページや他拡張が同じ window へ投げる message と
 * 混ざらない。value を unknown のままにしているのは、橋渡しが区画の中身を知らないため。
 */
type SettingsMessage = {
  readonly type: typeof SETTINGS_MESSAGE_TYPE;
  readonly key: string;
  readonly value: unknown;
};

/** 区画の配送メッセージを組む。 */
export const settingsMessage = <T>(
  section: SettingsSection<T>,
  value: T,
): SettingsMessage => ({
  type: SETTINGS_MESSAGE_TYPE,
  key: section.key,
  value,
});

const isSettingsMessage = (data: unknown): data is SettingsMessage => {
  const record = asUntrustedRecord(data);
  return record.type === SETTINGS_MESSAGE_TYPE && typeof record.key === "string";
};

/**
 * 受信データがこの区画への配送かを判定し、設定として取り出す。無関係なら undefined。
 *
 * 名前空間とキーが一致しても、message はページからも投げられるので値は信用しない。
 * 区画の normalize を通してから返す。
 */
export const settingFromMessage = <T>(
  section: SettingsSection<T>,
  data: unknown,
): T | undefined => {
  if (!isSettingsMessage(data) || data.key !== section.key) return undefined;
  return section.normalize(data.value);
};

/** 受け入れ判定に使う message の素性。MessageEvent をそのまま渡せる形にしてある。 */
type DeliverySource = {
  readonly source: unknown;
  readonly origin: string;
};

/**
 * 同一フレームの ISOLATED world からの配送かを判定する。
 *
 * message は埋め込みフレーム（ライブチャットの iframe）やページの JS からも同じ window へ届く。
 * 送信元 window の同一性と origin の一致の両方で、自フレーム由来だけを受ける。
 */
export const isSameFrameDelivery = (
  event: DeliverySource,
  frame: unknown,
  origin: string,
): boolean => event.source === frame && event.origin === origin;

/** ISOLATED world から MAIN world へ区画を配送する。 */
export const publishSection = <T>(
  section: SettingsSection<T>,
  value: T,
): void => {
  window.postMessage(settingsMessage(section, value), location.origin);
};

/**
 * MAIN world で区画の到着を購読する。
 *
 * 到着までは section.defaults で動かすこと。両 world の content script はどちらも document_start で
 * 走るため注入順に保証がなく、初回の配送を取りこぼし得る（配送側は遷移ごとに配り直す）。
 */
export const subscribeSection = <T>(
  section: SettingsSection<T>,
  onSetting: (value: T) => void,
): (() => void) => {
  const listener = (event: MessageEvent): void => {
    if (!isSameFrameDelivery(event, window, location.origin)) return;
    const value = settingFromMessage(section, event.data);
    if (value === undefined) return;
    onSetting(value);
  };
  window.addEventListener("message", listener);
  return () => window.removeEventListener("message", listener);
};
