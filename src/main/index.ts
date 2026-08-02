import { surfaceOf } from "./surface";

// 足場の確認用の唯一の振る舞い。傍受層・変換ロジックは後続で入る。
// readyState を出すのは、run_at: "document_start" で本当に走ったか（= loading か）を
// ログ 1 行で判定できるようにするため。
console.info(
  `[youtube-super-lite] injected: surface=${surfaceOf(location.pathname)} readyState=${document.readyState}`,
);
