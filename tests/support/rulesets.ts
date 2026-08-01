import { readFileSync } from "node:fs";
import path from "node:path";

import { manifestJson } from "./manifest";
import { matchesUrlFilter } from "./url-filter";

/**
 * manifest.json とそこから参照される静的 ruleset を読む。
 *
 * `rule_resources[].path` は拡張ルート相対で、この構成ではリポジトリルートと一致する
 * （manifest.json がルートに置かれ、build が `rules/` をそのまま dist へ写す）。
 * したがってルート起点で解決してよい。
 */
const root = path.resolve(import.meta.dir, "..", "..");

const readJson = (relativePath: string): unknown =>
  JSON.parse(readFileSync(path.join(root, relativePath), "utf8"));

export type RuleResource = {
  readonly id: string;
  readonly enabled: boolean;
  readonly path: string;
};

export type DnrRule = {
  readonly id: number;
  readonly priority?: number;
  readonly action?: { readonly type?: string };
  readonly condition?: {
    readonly urlFilter?: string;
    readonly regexFilter?: string;
    readonly resourceTypes?: readonly string[];
  };
};

export type StaticRuleset = RuleResource & { readonly rules: readonly DnrRule[] };

type Manifest = {
  readonly declarative_net_request?: {
    readonly rule_resources?: readonly RuleResource[];
  };
};

/** manifest の読み口は 1 つ。ここは ruleset の解決だけを足す。 */
export { manifestJson };

const manifest = manifestJson as unknown as Manifest;

export const ruleResources = (): readonly RuleResource[] => {
  const resources = manifest.declarative_net_request?.rule_resources;
  if (resources === undefined) {
    throw new Error(
      "manifest.json に declarative_net_request.rule_resources が無い",
    );
  }
  return resources;
};

export const loadStaticRulesets = (): readonly StaticRuleset[] =>
  ruleResources().map((resource) => ({
    ...resource,
    rules: readJson(resource.path) as readonly DnrRule[],
  }));

/** 製品構成でそのまま効くルール。要件 (b) の保護は必ずこの集合に対して検査する。 */
export const enabledRules = (): readonly DnrRule[] =>
  loadStaticRulesets()
    .filter((ruleset) => ruleset.enabled)
    .flatMap((ruleset) => ruleset.rules);

export const rulesetById = (id: string): StaticRuleset => {
  const ruleset = loadStaticRulesets().find((candidate) => candidate.id === id);
  if (ruleset === undefined) throw new Error(`ruleset が無い: ${id}`);
  return ruleset;
};

/**
 * `url` に一致するルールを、失敗時に原因が読める文字列で返す。
 *
 * `resourceTypes` は照合に使わない。使わない方が「一致した」と判定される範囲が広くなり、
 * 遮断してはならない URL の検査が厳しくなるため。逆に (a) の検査では
 * 「URL 集合として届いている」ことしか示せない。
 */
export const matchesOf = (
  rules: readonly DnrRule[],
  url: string,
): readonly string[] =>
  rules
    .filter((rule) => {
      const urlFilter = rule.condition?.urlFilter;
      return urlFilter !== undefined && matchesUrlFilter(urlFilter, url);
    })
    .map((rule) => `#${rule.id} ${rule.condition?.urlFilter ?? ""}`);
