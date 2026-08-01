import { publishSection } from "../shared/bridge";
import { onNavigated } from "../shared/navigation";
import {
  chatDisplaySection,
  localSettingsStore,
  readSection,
  watchSection,
  type SettingsSection,
} from "../shared/settings";
import { applyChatDisplay } from "./chat-display";

/**
 * ISOLATED world の入口。
 *
 * 拡張 API（chrome.storage）に触れられるのは MAIN world ではなくこちらなので、設定の読み出しと
 * MAIN world への配送を担う。機能そのもの（R2 / R3 / R4 / R5 の処理）はここに書かない。
 *
 * DOM を対象にする処理の再適用が必要な機能（R3 / R5）は、shared/navigation の onNavigated へ
 * 自分の適用処理を登録する。SPA 遷移の購読の仕組みはそこに置いてある。
 */

/**
 * MAIN world へ配る区画。設定を MAIN 側で使う機能（R4 など）はここへ自分の区画を足す。
 *
 * 保存されている全区画（service worker 側の一覧）とは別物で、こちらは MAIN world で必要な分だけに
 * 絞る。配送経路はページの JS から観測できるので、載せる情報を最小限に保つため。
 */
const deliveredSections: readonly SettingsSection<unknown>[] = [chatDisplaySection];

const deliverAll = async (): Promise<void> => {
  for (const section of deliveredSections) {
    publishSection(section, await readSection(localSettingsStore, section));
  }
};

void deliverAll();

// 変更通知は storage.onChanged が content script へ直接届くので、service worker を経由せず配り直す。
for (const section of deliveredSections) {
  watchSection(localSettingsStore, section, (value) =>
    publishSection(section, value),
  );
}

// 遷移ごとに配り直す。両 world の content script は注入順が保証されないため初回配送を
// 取りこぼし得るが、MAIN 側は到着まで既定値で動き、次の遷移で追いつく。
onNavigated(() => {
  void deliverAll();
});

/**
 * R5 のチャット表示。DOM へ CSS を当てるので MAIN world へは配らず、この world で当てる。
 *
 * watch ページとライブチャットの iframe の両方でこの content script が走り、それぞれが
 * 自分の文書へ当てる。どちらの規則が効くかは CSS のセレクタが決めるので、面（watch か
 * live_chat か）をここで見分けない。
 */
const applyChatDisplayFromStorage = async (): Promise<void> => {
  applyChatDisplay(await readSection(localSettingsStore, chatDisplaySection));
};

void applyChatDisplayFromStorage();

watchSection(localSettingsStore, chatDisplaySection, applyChatDisplay);

// 遷移では文書が作り直されないので、当てた CSS が残っているとは限らない。当て直す。
onNavigated(() => {
  void applyChatDisplayFromStorage();
});
