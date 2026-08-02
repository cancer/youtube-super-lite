import { readFileSync } from "node:fs";
import path from "node:path";

/**
 * manifest.json を読む。
 *
 * manifest は拡張の宣言なので、コード側の判定（surfaceOf・静的 ruleset）と統合できない。
 * 対応が保てているかはテストでしか見られないため、読み口をここに 1 つ置く。
 */
const root = path.resolve(import.meta.dir, "..", "..");

/** 型は宣言だが検証はしていない。形の検査は各テストが担う。 */
export const manifestJson = JSON.parse(
  readFileSync(path.join(root, "manifest.json"), "utf8"),
) as Record<string, unknown>;

type ContentScript = {
  readonly matches?: readonly string[];
};

const contentScripts = (): readonly ContentScript[] => {
  const scripts = manifestJson.content_scripts;
  if (!Array.isArray(scripts)) {
    throw new Error("manifest.json に content_scripts が無い");
  }
  return scripts as readonly ContentScript[];
};

/** content script が注入される match pattern。重複（world ごとの同一宣言）は畳む。 */
export const contentScriptMatches = (): readonly string[] => [
  ...new Set(contentScripts().flatMap((script) => script.matches ?? [])),
];

/**
 * match pattern から、そこに注入されるページの pathname を 1 つ作る。
 *
 * 末尾の `*` は「以降が何であってもよい」なので、最短の代表として取り除いた形を使う。
 * `https://www.youtube.com/watch*` なら `/watch`。
 */
export const injectedPathnames = (): readonly string[] =>
  contentScriptMatches().map((pattern) =>
    new URL(pattern.replace(/\*$/, "")).pathname,
  );
