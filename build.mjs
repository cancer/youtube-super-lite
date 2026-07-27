import { cp, mkdir, rm } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { build } from "esbuild";

const root = path.dirname(fileURLToPath(import.meta.url));
const outDir = path.join(root, "dist");

/**
 * 変換せずに dist へ置くファイル・ディレクトリ（root 相対）。
 *
 * `rules` は declarativeNetRequest の静的 ruleset。manifest の `path` は拡張ルート相対なので、
 * リポジトリ上の配置をそのまま dist に写す必要がある（バンドル対象ではない）。
 */
const staticPaths = ["manifest.json", "rules"];

await rm(outDir, { recursive: true, force: true });
await mkdir(outDir, { recursive: true });

await build({
  // 出力名は manifest の `js` と対応する（main → dist/main.js）。
  entryPoints: { main: path.join(root, "src/main/index.ts") },
  outdir: outDir,
  bundle: true,
  // content script は ES モジュールとして読み込めないので、エントリごとに単一ファイルの IIFE にする。
  // ESM 出力・コード分割・動的 import はいずれも不可。
  format: "iife",
  platform: "browser",
  target: "chrome120",
  // MAIN world に注入された content script にはページ（YouTube）の CSP が適用されるため、
  // eval を使う sourcemap は落ちる。external なら .map を別ファイルに出すだけで eval を踏まない。
  sourcemap: "external",
  logLevel: "info",
});

await Promise.all(
  staticPaths.map((target) =>
    cp(path.join(root, target), path.join(outDir, target), {
      recursive: true,
      // ルールの根拠を書いた README は実行時に使われない。配布物に混ぜない。
      filter: (source) => !source.endsWith(".md"),
    }),
  ),
);
