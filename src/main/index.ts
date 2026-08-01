import { registerChatImages } from "./chat-images";
import { installIntercept } from "./intercept";
import { surfaceOf } from "./surface";

// ページの JS が最初のリクエストを出す前に差し替えを終える必要があるので、
// 何よりも先に組み込む。R2（コメント・関連動画）の変換は後続でここに足す。
registerChatImages(installIntercept());

// 足場の確認用の唯一の振る舞い。変換ロジックは後続で入る。
// readyState を出すのは、run_at: "document_start" で本当に走ったか（= loading か）を
// ログ 1 行で判定できるようにするため。
console.info(
  `[youtube-super-lite] injected: surface=${surfaceOf(location.pathname)} readyState=${document.readyState}`,
);
