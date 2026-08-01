import type { WatchDeclutterSettings } from "../shared/settings";

/**
 * watch ページから「次の動画」の列とコメント欄を消す（要件 R2）。
 *
 * データ応答へ介入せず、構築された DOM からノードを外す。`display: none` では DOM も
 * リスナーも残り、狙い（描画とメモリの削減）に届かないので、必ず実際に削除する。
 *
 * DOM そのものではなく「セレクタで探して消せる根」だけを受け取るので、判断（何を消すか）は
 * ブラウザ無しで検証できる。実 DOM に当たるかどうかはセレクタ文字列の正しさの問題として分離する。
 */

/** 消せるノード。remove 以外を要求しないので、テストは素の値を渡せる。 */
export type RemovableNode = {
  remove(): void;
};

/** セレクタで探せる根。実体は document。 */
export type ElementRoot = {
  querySelectorAll(selectors: string): Iterable<RemovableNode>;
};

/**
 * まとめて消す 1 かたまり。
 *
 * name は消せたかどうかを外から報告するための識別子で、腐食（YouTube の DOM 変更でセレクタが
 * 当たらなくなること）を人が読めるログに出すために要る。
 */
export type RemovalGroup = {
  readonly name: string;
  readonly selectors: readonly string[];
};

/**
 * 右側の「次の動画」の列。
 *
 * ウィンドウ幅で `#secondary-inner` の下と動画の下（`#primary-inner > #below`）を行き来するため、
 * 列そのものではなく中身の入れ物である `#related` を名指しする。ライブチャット（`#chat-container`）・
 * 再生リスト（`#playlist`）・エンゲージメントパネル（`#panels`）は `#related` の兄弟なので残る。
 */
export const NEXT_VIDEOS_GROUP: RemovalGroup = {
  name: "next-videos",
  selectors: ["ytd-watch-flexy #related"],
};

/**
 * コメント欄。
 *
 * 動画の下の本体（`ytd-comments#comments`）、読み込み前に場所を取る抜け殻
 * （`#comments-leave-behind`）、パネルとして開くときの入れ物（target-id で名指しできる
 * エンゲージメントパネル）の 3 つで 1 かたまり。いずれもコメント専用の id / target-id なので、
 * タイトルとチャンネル情報（`ytd-watch-metadata`）や高評価は含まない。
 */
export const COMMENTS_GROUP: RemovalGroup = {
  name: "comments",
  selectors: [
    "ytd-watch-flexy ytd-comments#comments",
    "ytd-watch-flexy #comments-leave-behind",
    'ytd-watch-flexy ytd-engagement-panel-section-list-renderer[target-id="engagement-panel-comments-section"]',
  ],
};

/** 設定に応じて消す塊。「次の動画」の列は設定を持たず常に消す。 */
export const removalGroupsFor = (
  settings: WatchDeclutterSettings,
): readonly RemovalGroup[] =>
  settings.removeComments
    ? [NEXT_VIDEOS_GROUP, COMMENTS_GROUP]
    : [NEXT_VIDEOS_GROUP];

/** 1 かたまりを消し、1 つでも消せたかを返す。 */
const removeGroup = (root: ElementRoot, group: RemovalGroup): boolean => {
  let removedAny = false;
  for (const selector of group.selectors) {
    // 消す前に列挙を確定させる。querySelectorAll は静的な NodeList を返すので、
    // 途中で親を消しても列挙が壊れない（外れた子への remove は何も起きない）。
    for (const node of [...root.querySelectorAll(selector)]) {
      node.remove();
      removedAny = true;
    }
  }
  return removedAny;
};

/**
 * 設定に沿って消し、実際に消せた塊の名前を返す。
 *
 * 対象が 1 つも無い呼び出し（対象が挿入される前・既に消した後）は空の結果を返すだけで、
 * 例外を投げない。腐食しても視聴だけは続けられる、という要件をここで担保する。
 */
export const applyDeclutter = (
  root: ElementRoot,
  settings: WatchDeclutterSettings,
): readonly string[] =>
  removalGroupsFor(settings)
    .filter((group) => removeGroup(root, group))
    .map((group) => group.name);

/**
 * 消すはずなのに一度も消せていない塊の名前。
 *
 * セレクタが腐食すると「消えないまま静かに動き続ける」状態になるため、これを呼び出し側が
 * ログに出して気づけるようにする。
 */
export const unmatchedGroupNames = (
  settings: WatchDeclutterSettings,
  removedNames: Iterable<string>,
): readonly string[] => {
  const removed = new Set(removedNames);
  return removalGroupsFor(settings)
    .map((group) => group.name)
    .filter((name) => !removed.has(name));
};
