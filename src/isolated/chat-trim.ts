/**
 * チャット項目の保持数に上限を設ける（要件 R3 の DOM 層）。
 *
 * 画像を作らせない変換（MAIN world の chat-images）で塞げるのは画像の蓄積だけで、発言のノード自体は
 * 増え続ける。長時間の配信ではこれが定常メモリ増加の残りの経路になるので、YouTube 自身の保持数より
 * 手前で古い項目を捨てる。
 *
 * 消すのは古い側の項目だけで、発言の中身には触らない。
 */

/**
 * DOM に残すチャット項目の数。
 *
 * 要件は「YouTube 自身の上限より厳しく」としか定めていない。表示に必要なのは画面に見えている
 * 数十件とその少し上までなので、そこを数画面分上回る値を採る。設定にはしない（効きが足りなければ
 * この定数だけを動かす）。
 */
export const CHAT_ITEM_LIMIT = 100;

/**
 * 上限を当て直す間隔（ミリ秒）。
 *
 * 上限は定常状態のメモリを抑えるためのもので、一瞬でも超えてはならない類の不変条件ではない。
 * 発言の追加を監視して即座に削ることもできるが、監視には対象要素の出現待ちと遷移後の張り直しという
 * 状態が要る。定期的に引き直すだけなら、その状態を持たずに同じ効果が得られる。
 */
export const CHAT_TRIM_INTERVAL_MS = 2000;

/**
 * 上限を当てる対象。実体はチャット項目の親要素で、テストではフェイクを渡す。
 *
 * 件数・先頭・除去の 3 つしか使わないことを型で示す。項目そのものの中身は見ない。
 */
export type ChatItemList = {
  readonly childElementCount: number;
  readonly firstElementChild: { remove: () => void } | null;
};

/** チャット項目の親要素を指すセレクタ。 */
export const CHAT_ITEM_LIST_SELECTOR = "yt-live-chat-item-list-renderer #items";

/**
 * 超過分を古い側から外す。
 *
 * 適用先が `null` で来るのは、チャットの描画がまだで要素が無い場合と、YouTube の DOM 構造が
 * 変わってセレクタが外れた場合。どちらも「上限が効かないだけ」に留める。
 *
 * 回す回数を先に確定させるのは、除去が件数へ反映されなかったときに終わらなくなるのを避けるため。
 */
export const trimChatItems = (list: ChatItemList | null, limit: number): void => {
  if (list === null) return;
  for (let excess = list.childElementCount - limit; excess > 0; excess -= 1) {
    list.firstElementChild?.remove();
  }
};

/** 周期処理の登録口。実体は setInterval で、テストではフェイクを渡す。 */
export type Scheduler = (task: () => void, intervalMs: number) => void;

/**
 * 上限の適用を始める。
 *
 * 適用先は毎回 `findList` で引き直す。要素は Polymer が後から作り、遷移でも作り直されるので、
 * 掴んだ参照は持ち越せない。引き直しさえすれば遷移後の再適用も同じ経路で済む。
 */
export const startChatTrim = (
  findList: () => ChatItemList | null,
  schedule: Scheduler = setInterval,
): void => {
  schedule(() => trimChatItems(findList(), CHAT_ITEM_LIMIT), CHAT_TRIM_INTERVAL_MS);
};
