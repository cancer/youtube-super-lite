import { cp, mkdir, rm } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { build } from "esbuild";

const root = path.dirname(fileURLToPath(import.meta.url));
const outDir = path.join(root, "dist");

/** 変換せずに dist へ置くファイル（root 相対）。 */
const staticFiles = ["manifest.json"];

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
  staticFiles.map((file) => cp(path.join(root, file), path.join(outDir, file))),
);
