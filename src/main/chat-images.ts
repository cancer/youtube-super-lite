import type { JsonTransform, TransformRegistry } from "./intercept";

/**
 * ライブチャット応答から視聴者アイコンを落とす変換（要件 R3）。
 *
 * DOM から `<img>` を消すのでは遅い。要件の受け入れ条件は「表示されない」ではなく
 * 「画像リクエストも発生しない」なので、`<img>` が作られる前、つまり応答の JSON から
 * 画像 URL を消してページへ渡す。
 *
 * **落とす基準は「装飾かどうか」ではなく「種類が増え続けるかどうか」**（ユーザー決定 2026-08-02）。
 * この機能が抑えたいのは長時間視聴での定常増加であって、1 回あたりの重さではない。視聴者アイコンは
 * 発言者が変わるたびに別の URL が来るので、配信が続くかぎり取得もデコード結果も増え続ける。対して
 * 絵文字・メンバースタンプ・有料スタンプ・ギフト画像・チャンネル所有者のアイコン・メンバーバッジは
 * 配信ごとに決まった枚数を使い回すだけで、視聴時間に対して増えない。よって落とすのは前者だけとする。
 *
 * 走査は経路の列挙ではなく木全体の再帰で行う。理由は 3 つある。
 * - 同じ renderer が別の場所に入れ子で現れる（ティッカーは有料項目の renderer を丸ごと持ち直し、
 *   ピン留めは addChatItemAction の外に発言を置く）。経路を列挙すると必ず取りこぼす
 * - 配信中（get_live_chat）とアーカイブ（get_live_chat_replay）で actions[] の階層が違う。
 *   replay は replayChatItemAction.actions[] の分だけ深い。再帰なら外形の差を知らずに済む
 * - 実測できていない項目種別（メンバー加入・ギフト購入告知など）が残っている。経路で書くと
 *   種別が増えるたびに実装が増えるが、鍵で書けば同じ鍵を使う限り自動で掛かる
 *
 * 落とすのは画像 URL の配列だけで、入れ物は残す。代替テキスト（アバターの alt）が入れ物側にあり、
 * 誰の発言かを保つのに要るため。
 *
 * この変換は初回バッチには掛からない。チャットの最初の ~100 件は fetch を通らず
 * iframe の HTML に ytInitialData として埋め込まれて届くため、そのぶんのアバターは残る。
 */

/** 画像 URL の配列が入る鍵。`thumbnails` は *Renderer 系、`sources` は ViewModel 系が使う。 */
const IMAGE_URL_KEYS: ReadonlySet<string> = new Set(["thumbnails", "sources"]);

/**
 * 中の画像を無条件に落とす鍵。値の下にある画像 URL の配列をすべて消す。
 *
 * いずれも投稿者・送信者のアイコンが入る鍵で、視聴者が変わるたびに別の URL が来る。
 *
 * `authorPhoto` はここに無い。同じ視聴者アイコンだが、投稿者のバッジ次第で残すため別の規則で扱う。
 * 逆に、ここに挙げた鍵はバッジを見ない。投稿者がモデレーターやオーナーであっても落ちる。バッジを
 * 見る例外は `authorPhoto` の鍵名を指して定めたもので、他の鍵へは広げていない。取りこぼしではなく
 * 決定なので、モデレーターでも落ちることをテストで固定してある。
 *
 * 絵文字（`emoji`）・有料スタンプ（`sticker`）・ギフト画像（`giftImage`）・❤ のチャンネル所有者
 * アイコン（`creatorThumbnail`）はここに無い。2026-08-02 までは装飾として落としていたが、
 * どれも配信あたりの種類が有限で視聴時間に対して増えないため、落とす基準から外れた。
 * 鍵を足すときは、増え続けるものかどうかで決めること。
 */
const STRIPPED_KEYS: ReadonlySet<string> = new Set([
  // ギフト送信者のアバター。ViewModel 側の鍵で、authorPhoto ではない。
  "authorAvatar",
  // ティッカーの投稿者アバター。これも authorPhoto ではない。
  "sponsorPhoto",
]);

/** `authorPhoto` を残す投稿者。要件が定めた表示対象（モデレーターとチャンネル所有者）。 */
const AVATAR_ICON_TYPES: ReadonlySet<string> = new Set(["MODERATOR", "OWNER"]);

/**
 * 配列でないオブジェクトか。
 *
 * 応答は型システムの外から来るので、鍵を引く前に必ずこれを通す。
 */
const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const fieldOf = (node: unknown, key: string): unknown =>
  isRecord(node) ? node[key] : undefined;

/**
 * `authorBadges` の値から、その投稿者のアバターを表示するかを決める。
 *
 * 「誰のアバターを出すか」は要件がユーザーの決定として持つ仕様なので、除去の手続きから切り離して
 * ここだけで決める。判定に使うのはバッジだけで、投稿の種類（発言・スーパーチャット・参加者一覧）には
 * よらない。
 *
 * バッジは配列で複数入る（実測で [VERIFIED, MODERATOR]）。先頭だけを見るとモデレーターを
 * 取りこぼすので全要素を走る。メンバーバッジは `icon` を持たず `customThumbnail` で来るため、
 * `iconType` を見るこの判定では表示の根拠にならない。
 *
 * 値は応答そのままで型の保証が無いので、配列でなければ表示しない側へ倒す。
 */
export const showsAuthorPhoto = (authorBadges: unknown): boolean =>
  Array.isArray(authorBadges) &&
  authorBadges.some((badge) => {
    const renderer = fieldOf(badge, "liveChatAuthorBadgeRenderer");
    const iconType = fieldOf(fieldOf(renderer, "icon"), "iconType");
    return typeof iconType === "string" && AVATAR_ICON_TYPES.has(iconType);
  });

/** subtree から画像 URL の配列だけを消す。入れ物と、そこに載る代替テキストは残す。 */
const stripImageUrls = (node: unknown): void => {
  if (Array.isArray(node)) {
    for (const element of node) stripImageUrls(element);
    return;
  }
  if (!isRecord(node)) return;
  for (const [key, value] of Object.entries(node)) {
    if (IMAGE_URL_KEYS.has(key)) delete node[key];
    else stripImageUrls(value);
  }
};

const walk = (node: unknown): void => {
  if (Array.isArray(node)) {
    for (const element of node) walk(element);
    return;
  }
  if (!isRecord(node)) return;

  // 表示対象外なら投稿者アバターを丸ごと落とす。鍵ごと消すので `<img>` が作られない。
  if ("authorPhoto" in node && !showsAuthorPhoto(node.authorBadges)) {
    delete node.authorPhoto;
  }

  for (const [key, value] of Object.entries(node)) {
    if (STRIPPED_KEYS.has(key)) stripImageUrls(value);
    else walk(value);
  }
};

/**
 * 応答の木を書き換えて返す。
 *
 * 複製を作らず元の木を書き換えるのは、この機能の目的が長時間視聴での定常メモリ増加の抑制で、
 * ポーリングのたびに応答 1 本分のコピーを作るのが目的に反するため。
 *
 * 書き換えて安全な理由は傍受層の 3 経路で異なる。fetch と XHR の responseText では、渡る木は
 * この変換のために起こしたもの（clone した応答／生文字列のパース結果）なのでページ側と共有しない。
 * XHR の response（responseType: "json"）だけは XHR 自身が持つ木を直に書き換えるが、ページは
 * この変換を通した値しか受け取らないので食い違わない。処理は鍵を消すだけで、同じ木に何度当てても
 * 結果が変わらないため、getter が繰り返し呼ばれても壊れない。
 */
export const stripChatImages: JsonTransform = (json) => {
  walk(json);
  return json;
};

/**
 * この変換を傍受層へ繋ぐ。
 *
 * どのエンドポイントを担当するかは変換の側の知識なので、入口（MAIN world の index）ではなく
 * ここに置く。入口は「R3 を組み込む」とだけ書けばよくなる。
 */
export const registerChatImages = (registry: TransformRegistry): void => {
  registry.register("live_chat", stripChatImages);
};
