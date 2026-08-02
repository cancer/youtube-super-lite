import { onNavigated } from "../shared/navigation";
import {
  applySection,
  localSettingsStore,
  watchDeclutterSection,
  watchSection,
  type SettingsStore,
  type WatchDeclutterSettings,
} from "../shared/settings";
import {
  applyDeclutter,
  unmatchedGroupNames,
  type ElementRoot,
} from "./declutter";

/**
 * watch ページの整理（要件 R2）を、設定・実 DOM・再適用の契機へ繋ぐ。
 *
 * 何を消すかは declutter が持ち、ここは「いつ消すか」だけを決める。副作用の相手（storage・DOM・
 * 監視・タイマー）はすべて差し替えられる形で受け取るので、順序の保証をブラウザ無しで検証できる。
 */

/**
 * セレクタが 1 つも当たらないまま黙って動き続けるのを避けるための報告までの待ち時間。
 *
 * 「まだ挿入されていない」と「もう当たらない（腐食）」は見分けられないので、挿入を待ち切れる
 * だけ待ってから 1 度だけ見る。1 度でも当たれば以降も当たるため、遷移ごとに繰り返す必要はない。
 */
export const CORROSION_REPORT_MS = 30_000;

/** MutationObserver が渡す変更のうち、追加の有無だけを見るための形。 */
type AdditionRecord = {
  readonly addedNodes: { readonly length: number };
};

/**
 * 変更の中に追加が含まれるか。
 *
 * 追加が 1 つも無い変更（属性・文字列の更新や、削除だけの変更）では探し直さない。watch ページの
 * 変更の大半はこちらで、探すだけ無駄になる。自分の削除もここで弾かれるので、消したことが
 * 次の探索を呼ぶ連鎖にならない。
 */
export const hasAddedNodes = (records: readonly AdditionRecord[]): boolean =>
  records.some((record) => record.addedNodes.length > 0);

/** DOM への追加を購読する。実体は MutationObserver で、テストではフェイクを渡す。 */
export type AdditionWatcher = (onAdded: () => void) => void;

/** 待ち時間を置いて 1 度だけ走らせる。実体は setTimeout。 */
export type DelayedTask = (task: () => void, delayMs: number) => void;

/**
 * 実文書への追加を購読する。
 *
 * 対象は document_start の時点では存在せず、遷移でも作り直される。出現を捕まえるには待つしか
 * ないので childList を購読し続ける。監視を打ち切ると、後から差し込まれた分（読み込みが遅れた
 * コメント欄など）を取りこぼす。
 *
 * 見る文書は `root` と同じ実文書だが、`root` は「セレクタで探せる」ことしか要求しない型なので
 * ここからは辿れない。`root` を差し替えるなら、その文書を見る `watchAdditions` も併せて渡すこと。
 */
const observeDocumentAdditions: AdditionWatcher = (onAdded) => {
  new MutationObserver((records) => {
    if (hasAddedNodes(records)) onAdded();
  }).observe(document.documentElement, { childList: true, subtree: true });
};

const debug = (message: string): void => {
  console.debug(`[youtube-super-lite] ${message}`);
};

/** 繋ぎ先。既定は実ブラウザのもので、テストはすべて差し替える。 */
export type WatchDeclutterOptions = {
  readonly store?: SettingsStore;
  readonly root?: ElementRoot;
  readonly watchAdditions?: AdditionWatcher;
  readonly navigate?: (apply: () => void) => void;
  readonly delay?: DelayedTask;
  readonly report?: (message: string) => void;
};

export const installWatchDeclutter = ({
  store = localSettingsStore,
  root = document,
  watchAdditions = observeDocumentAdditions,
  navigate = onNavigated,
  delay = setTimeout,
  report = debug,
}: WatchDeclutterOptions = {}): void => {
  let settings: WatchDeclutterSettings = watchDeclutterSection.defaults;
  const removedGroups = new Set<string>();

  const apply = (): void => {
    for (const name of applyDeclutter(root, settings)) {
      removedGroups.add(name);
    }
  };

  // 既定は「消す」なので、保存値が届く前に当てると「残す」を選んでいる人のコメント欄まで
  // 消してしまう（消したノードは戻せない）。最初の適用は必ず読み出しの後。
  // 適用の契機（遷移・DOM への追加・腐食の報告）もすべてここより後に張る。
  // 失効して保存値が読めないときも同じ理由で何も消さない（契機ごと張らない）。
  void applySection(store, watchDeclutterSection, (stored) => {
    settings = stored;
    navigate(apply);
    watchAdditions(apply);
    delay(() => {
      const unmatched = unmatchedGroupNames(settings, removedGroups);
      if (unmatched.length === 0) return;
      report(`watch ページから消せなかった: ${unmatched.join(", ")}`);
    }, CORROSION_REPORT_MS);
  });

  // 「消す」へ切り替えたときはその場で消す。「残す」へ戻したぶんは消したノードを作り直せないので、
  // 次に開いた watch ページから効く（サイドパネルにその旨を出してある）。
  watchSection(store, watchDeclutterSection, (changed) => {
    settings = changed;
    apply();
  });
};
