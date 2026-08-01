import { installEqualizer } from "./audio-graph";
import { installIntercept } from "./intercept";
import { surfaceOf } from "./surface";

// ページの JS が最初のリクエストを出す前に差し替えを終える必要があるので、
// 何よりも先に組み込む。変換関数（R2 / R3）は後続でここに登録する。
installIntercept();

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
