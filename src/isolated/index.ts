import { startChatDisplay } from "./chat-display";
import { CHAT_ITEM_LIST_SELECTOR, startChatTrim } from "./chat-trim";
import { startSettingsDelivery } from "./deliver";
import { installWatchDeclutter } from "./watch-declutter";

/**
 * ISOLATED world の入口。
 *
 * 拡張 API（chrome.storage）に触れられるのは MAIN world ではなくこちらなので、設定の読み出しと
 * MAIN world への配送、および DOM を対象にする機能がここに集まる。
 *
 * 何をどう変えるかも、いつ変えるかも、それぞれの機能のモジュールが持つ。この入口は
 * 「どの面でどれを起動するか」だけを書く場所で、判断を足さない。
 */

// 設定を MAIN world（R4 のイコライザなど）へ配る。
startSettingsDelivery();

// watch ページから「次の動画」の列とコメント欄を消す（R2）。この content script は
// ライブチャットの iframe（/live_chat）にも入るが、整理の対象は watch ページだけ。
// 判定は manifest の matches と対応させてある。
if (location.pathname.startsWith("/watch")) installWatchDeclutter();

// チャット項目の上限を当て続ける（R3 の DOM 層）。watch ページにも注入されるが、
// そちらではセレクタが一致せず何も起きない。
startChatTrim(() => document.querySelector(CHAT_ITEM_LIST_SELECTOR));

// チャットの文字サイズとパネル幅（R5）。DOM へ CSS を当てるので MAIN world へは配らず、
// この world の各文書（watch とライブチャットの iframe）へ当てる。
startChatDisplay();
