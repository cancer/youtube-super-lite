import { publishSection } from "../shared/bridge";
import { equalizerSection } from "../shared/equalizer";
import { onNavigated } from "../shared/navigation";
import {
  chatDisplaySection,
  localSettingsStore,
  readSection,
  watchDeclutterSection,
  watchSection,
  type SettingsSection,
  type WatchDeclutterSettings,
} from "../shared/settings";
import { applyDeclutter, unmatchedGroupNames } from "./declutter";

/**
 * ISOLATED world の入口。
 *
 * 拡張 API（chrome.storage）に触れられるのは MAIN world ではなくこちらなので、設定の読み出しと
 * MAIN world への配送を担う。加えて、DOM を対象にする機能を「設定」と「適用の契機」へ繋ぐ。
 *
 * 繋ぐだけで、何をどう変えるかの判断は持たない（R2 なら消す対象は isolated/declutter）。
 * 再適用の契機のうち SPA 遷移は shared/navigation の onNavigated に置いてある。
 */

/**
 * MAIN world へ配る区画。設定を MAIN 側で使う機能（R4 など）はここへ自分の区画を足す。
 *
 * 保存されている全区画（service worker 側の一覧）とは別物で、こちらは MAIN world で必要な分だけに
 * 絞る。配送経路はページの JS から観測できるので、載せる情報を最小限に保つため。
 */
const deliveredSections: readonly SettingsSection<unknown>[] = [
  chatDisplaySection,
  equalizerSection,
];

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
 * セレクタが 1 つも当たらないまま黙って動き続けるのを避けるための報告までの待ち時間。
 *
 * 「まだ挿入されていない」と「もう当たらない（腐食）」は見分けられないので、挿入を待ち切れる
 * だけ待ってから 1 度だけ見る。1 度でも当たれば以降も当たるため、遷移ごとに繰り返す必要はない。
 */
const CORROSION_REPORT_MS = 30_000;

/**
 * watch ページから「次の動画」の列とコメント欄を消す（要件 R2）。
 *
 * 何を消すかは declutter が持ち、ここは実 DOM・設定・再適用の契機を繋ぐだけ。
 */
const installWatchDeclutter = (): void => {
  let settings: WatchDeclutterSettings = watchDeclutterSection.defaults;
  const removedGroups = new Set<string>();

  const apply = (): void => {
    for (const name of applyDeclutter(document, settings)) {
      removedGroups.add(name);
    }
  };

  // 対象は document_start の時点では存在せず、遷移でも作り直される。出現を捕まえるには待つしか
  // ないので childList を購読し続ける。監視を打ち切ると、後から差し込まれた分（読み込みが遅れた
  // コメント欄など）を取りこぼす。
  //
  // 追加が 1 つも無い変更（属性・文字列の更新や、削除だけの変更）では探し直さない。watch ページの
  // 変更の大半はこちらで、探すだけ無駄になる。自分の削除もここで弾かれるので、消したことが
  // 次の探索を呼ぶ連鎖にならない。
  const observer = new MutationObserver((records) => {
    if (records.some((record) => record.addedNodes.length > 0)) apply();
  });

  // 既定は「消す」なので、保存値が届く前に当てると「残す」を選んでいる人のコメント欄まで
  // 消してしまう（消したノードは戻せない）。最初の適用は必ず読み出しの後。
  void readSection(localSettingsStore, watchDeclutterSection).then((stored) => {
    settings = stored;
    onNavigated(apply);
    observer.observe(document.documentElement, {
      childList: true,
      subtree: true,
    });
    setTimeout(() => {
      const unmatched = unmatchedGroupNames(settings, removedGroups);
      if (unmatched.length === 0) return;
      console.debug(
        `[youtube-super-lite] watch ページから消せなかった: ${unmatched.join(", ")}`,
      );
    }, CORROSION_REPORT_MS);
  });

  // 「消す」へ切り替えたときはその場で消す。「残す」へ戻したぶんは消したノードを作り直せないので、
  // 次に開いた watch ページから効く（サイドパネルにその旨を出してある）。
  watchSection(localSettingsStore, watchDeclutterSection, (changed) => {
    settings = changed;
    apply();
  });
};

// この content script はライブチャットの iframe（/live_chat）にも入る。整理の対象は watch ページ
// だけなので、そちらでは監視を始めない。判定は manifest の matches と対応させてある。
if (location.pathname.startsWith("/watch")) {
  installWatchDeclutter();
}
