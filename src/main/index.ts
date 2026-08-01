import { installEqualizer } from "./audio-graph";
import { registerChatImages } from "./chat-images";
import { installIntercept } from "./intercept";
import { surfaceOf } from "../shared/surface";

// ページの JS が最初のリクエストを出す前に差し替えを終える必要があるので、
// 何よりも先に組み込む。R2（コメント・関連動画）の変換は後続でここに足す。
registerChatImages(installIntercept());

const surface = surfaceOf(location.pathname);

// R4 のイコライザは <video> を持つ面にだけ要る。同じ content script が入る
// ライブチャットの iframe には <video> が無いので、購読も監視も張らない。
if (surface === "watch") installEqualizer();

// 足場の確認用の唯一の振る舞い。変換ロジックは後続で入る。
// readyState を出すのは、run_at: "document_start" で本当に走ったか（= loading か）を
// ログ 1 行で判定できるようにするため。
console.info(
  `[youtube-super-lite] injected: surface=${surface} readyState=${document.readyState}`,
);
