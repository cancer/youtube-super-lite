import { publishSection } from "../shared/bridge";
import { equalizerSection } from "../shared/equalizer";
import { onNavigated } from "../shared/navigation";
import {
  chatDisplaySection,
  localSettingsStore,
  readSection,
  watchSection,
  type SettingsSection,
  type SettingsStore,
} from "../shared/settings";

/**
 * 設定を ISOLATED world から MAIN world へ配る。
 *
 * 拡張 API（chrome.storage）に触れられるのは MAIN world ではなくこちらなので、読み出しと配送は
 * この world が担う。配る中身の組み立てと受け取りは shared/bridge が持ち、ここは
 * 「いつ・どの区画を配るか」だけを決める。
 */

/**
 * MAIN world へ配る区画。設定を MAIN 側で使う機能（R4 など）はここへ自分の区画を足す。
 *
 * 保存されている全区画（service worker 側の一覧）とは別物で、こちらは MAIN world で必要な分だけに
 * 絞る。配送経路はページの JS から観測できるので、載せる情報を最小限に保つため。
 */
export const DELIVERED_SECTIONS: readonly SettingsSection<unknown>[] = [
  chatDisplaySection,
  equalizerSection,
];

/** 区画を MAIN world へ送る。実体は window.postMessage 経由の publishSection。 */
export type SectionPublisher = <T>(section: SettingsSection<T>, value: T) => void;

/** 繋ぎ先。既定は実ブラウザのもので、テストはすべて差し替える。 */
export type SettingsDeliveryOptions = {
  readonly store?: SettingsStore;
  readonly sections?: readonly SettingsSection<unknown>[];
  readonly publish?: SectionPublisher;
  readonly navigate?: (deliver: () => void) => void;
};

export const startSettingsDelivery = ({
  store = localSettingsStore,
  sections = DELIVERED_SECTIONS,
  publish = publishSection,
  navigate = onNavigated,
}: SettingsDeliveryOptions = {}): void => {
  const deliverAll = async (): Promise<void> => {
    for (const section of sections) {
      publish(section, await readSection(store, section));
    }
  };

  // document_start では onNavigated が DOMContentLoaded まで初回を遅らせるので、そこを待たずに配る。
  void deliverAll();

  // 変更通知は storage.onChanged が content script へ直接届くので、service worker を経由せず配り直す。
  for (const section of sections) {
    watchSection(store, section, (value) => publish(section, value));
  }

  // 遷移ごとに配り直す。両 world の content script は注入順が保証されないため初回配送を
  // 取りこぼし得るが、MAIN 側は到着まで既定値で動き、次の遷移で追いつく。
  navigate(() => {
    void deliverAll();
  });
};
