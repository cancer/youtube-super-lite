import { equalizerSection } from "./equalizer";
import {
  chatDisplaySection,
  watchDeclutterSection,
  type SettingsSection,
} from "./settings";

/**
 * 設定を持つ区画の一覧。設定を持つ機能を足したら、ここへ自分の区画を足す。
 *
 * 一覧を要るのは区画の中身を知らない立場の側で、いずれも「全部に同じことをする」ためにこれを使う。
 * - 保存値の正規化（service worker）
 * - タブ単位の保存先を永続の保存値で埋める（content script とサイドパネル）
 *
 * MAIN world へ配る区画（isolated/deliver）とは別物。あちらは配送経路に載せる分だけに絞った一覧で、
 * 絞る理由も別にある。
 */
export const PERSISTED_SECTIONS: readonly SettingsSection<unknown>[] = [
  chatDisplaySection,
  watchDeclutterSection,
  equalizerSection,
];
