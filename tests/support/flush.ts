/**
 * 保留中の非同期処理が片付くまで待つ。
 *
 * 設定の読み出しは await を挟んで適用へ届くので、マイクロタスク 1 回では検査したい時点まで
 * 進まない。待つ長さではなく「保留が残っていない」ことが要るので、タスクの末尾まで送る。
 */
export const flush = (): Promise<void> =>
  new Promise((resolve) => setTimeout(resolve, 0));
