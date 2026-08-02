import { asUntrustedRecord } from "./settings";

/**
 * content script が「自分はどのタブか」を service worker に訊く経路。
 *
 * content script からタブ番号は見えない（chrome.tabs はページ側の world にも ISOLATED world にも
 * 無い）。一方 service worker は受け取ったメッセージの送り主としてタブを知っているので、訊いて
 * 答えてもらう形にする。
 *
 * この経路に載るのはタブ番号だけで、設定そのものは流さない。設定は storage を通じて共有する。
 */
export const TAB_ID_REQUEST = "youtube-super-lite/tab-id";

/** 問い合わせがこれかを判定する。他拡張や自分の別の用途のメッセージと混ざらないよう type で見る。 */
export const isTabIdRequest = (message: unknown): boolean =>
  asUntrustedRecord(message).type === TAB_ID_REQUEST;

/** 問い合わせの送り方。既定は実ブラウザのもので、テストは差し替える。 */
export type MessageSender = (message: unknown) => Promise<unknown>;

const sendToServiceWorker: MessageSender = (message) =>
  chrome.runtime.sendMessage(message);

/**
 * 自分のタブ番号を訊く。分からなければ undefined。
 *
 * 失敗を投げないのは、呼び出し側にできることが「タブ単位をあきらめて全タブ共通の保存先で動く」
 * しかないため。番号が取れない状況（拡張の再読み込みによる失効、service worker が答えられない）
 * はいずれもページを開き直すまで回復せず、再試行の意味も無い。
 */
export const requestTabId = async (
  send: MessageSender = sendToServiceWorker,
): Promise<number | undefined> => {
  try {
    const answer = await send({ type: TAB_ID_REQUEST });
    return typeof answer === "number" ? answer : undefined;
  } catch {
    return undefined;
  }
};
