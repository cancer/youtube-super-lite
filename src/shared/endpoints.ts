/**
 * URL の分類を 1 箇所に閉じるモジュール。
 *
 * 遮断（R1）と JSON 変換（R2 / R3）はどちらも「この URL は何か」の判定を必要とするが、
 * 判定を各機能に置くと同じ URL が別々の場所で別々に解釈され、要件が明記した
 * 「遮断してはならない系統」を守れなくなる。判定はこのモジュールだけが持つ。
 */

/**
 * R1 が定めるネットワーク遮断の方針。要件の 3 層に対応する。
 *
 * - `blockable`: (a) 純粋な計測。再生・認証・視聴履歴に関与しないので遮断してよい
 * - `protected`: (b) 機能・認証・視聴履歴に紐づく。遮断してはならない
 * - `unrestricted`: (a) (b) いずれでもない。遮断対象に含めない
 *
 * (c)（広告の配信・表示に必要なリクエスト）に固有の値を置かないのは、要件が (c) を
 * URL の列挙ではなく「表示に必要か」という境界で定義しているため。列挙できない層を
 * 型の値にすると実体のない分類が残るので、(c) は `unrestricted` の既定に含める。
 */
export type BlockPolicy = "blockable" | "protected" | "unrestricted";

/** JSON 応答の変換対象。`watch` は R2、`live_chat` は R3 が担当する。 */
export type TransformTarget = "watch" | "live_chat";

/**
 * (b) 遮断してはならない系統のパス。
 *
 * heartbeat はライブの配信状態のポーリング、att は bot 判定の通過、
 * playback / watchtime は視聴履歴の記録経路であり、いずれも計測ではない。
 */
const PROTECTED_PATHS: readonly string[] = [
  "/youtubei/v1/player/heartbeat",
  "/youtubei/v1/att/",
  "/api/stats/playback",
  "/api/stats/watchtime",
];

/**
 * (a) 遮断してよい系統のパス。
 *
 * 要件が (a) に挙げているもののうち、次の 2 つはここに含めない。
 * - 削除対象領域のサムネイル（`i.ytimg.com`）: 同じホストからプレーヤーのポスターも来るため、
 *   URL だけでは「削除対象の領域のサムネイルか」を判別できない
 * - 次動画のプリフェッチ / preconnect: 要件が具体的な URL を挙げていない
 * どちらも遮断ルールを作る側で実物を観測してから足す。
 */
const BLOCKABLE_PATHS: readonly string[] = [
  "/youtubei/v1/log_event",
  "/api/stats/qoe",
  "/api/stats/ads",
  "/api/stats/atr",
  "/ptracking",
  "/pagead/interaction/",
  "/pagead/viewthroughconversion/",
];

/**
 * 変換対象のパスと担当。
 *
 * `get_live_chat_replay` は `get_live_chat` の前方一致に含まれるが、同じ `live_chat` に
 * 落ちるので判定は変わらない。要件が挙げた 2 系統をそのまま写して読めるようにしてある。
 */
const TRANSFORM_PATHS: ReadonlyArray<readonly [string, TransformTarget]> = [
  ["/youtubei/v1/get_watch", "watch"],
  ["/youtubei/v1/live_chat/get_live_chat", "live_chat"],
  ["/youtubei/v1/live_chat/get_live_chat_replay", "live_chat"],
];

/** 相対 URL の解決に使う base。分類はすべてパスで行うのでホストは判定に使わない。 */
const BASE_URL = "https://www.youtube.com";

/**
 * URL 文字列から pathname を取り出す。
 *
 * ページの JS から渡る値なので URL として壊れていることがある。解釈できないときは
 * 空文字を返し、どの分類にも当たらない（＝遮断も変換もしない）方へ倒す。
 */
const pathOf = (url: string): string => {
  try {
    return new URL(url, BASE_URL).pathname;
  } catch {
    return "";
  }
};

/**
 * R1 の遮断方針を判定する。
 *
 * (b) を (a) より先に評価するのは、将来 (a) 側に広いパターンが足されても
 * 「遮断してはならない」系統が遮断対象へ落ちないようにするため。
 */
export const blockPolicyOf = (url: string): BlockPolicy => {
  const path = pathOf(url);
  if (PROTECTED_PATHS.some((prefix) => path.startsWith(prefix))) return "protected";
  if (BLOCKABLE_PATHS.some((prefix) => path.startsWith(prefix))) return "blockable";
  return "unrestricted";
};

/** JSON 変換の対象を判定する。対象でなければ undefined を返す。 */
export const transformTargetOf = (url: string): TransformTarget | undefined => {
  const path = pathOf(url);
  return TRANSFORM_PATHS.find(([prefix]) => path.startsWith(prefix))?.[1];
};
