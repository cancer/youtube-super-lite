import { describe, expect, test } from "bun:test";

import { manifestJson } from "./support/manifest";
import {
  enabledRules,
  loadStaticRulesets,
  matchesOf,
  ruleResources,
  rulesetById,
  type DnrRule,
} from "./support/rulesets";

/**
 * declarativeNetRequest の上限。出所は公式リファレンスの定数。
 * https://developer.chrome.com/docs/extensions/reference/api/declarativeNetRequest
 *
 * GUARANTEED_MINIMUM_STATIC_RULES は「他の拡張の状況に依らず必ず使える」下限であり、
 * これを超えた分はブラウザ全体のグローバル上限を食い合う。したがってここを実効上限として扱う。
 */
const GUARANTEED_MINIMUM_STATIC_RULES = 30_000;
const MAX_NUMBER_OF_STATIC_RULESETS = 100;
const MAX_NUMBER_OF_ENABLED_STATIC_RULESETS = 50;

const ACTION_TYPES = [
  "block",
  "redirect",
  "allow",
  "upgradeScheme",
  "modifyHeaders",
  "allowAllRequests",
];

const RESOURCE_TYPES = [
  "main_frame",
  "sub_frame",
  "stylesheet",
  "script",
  "image",
  "font",
  "object",
  "xmlhttprequest",
  "ping",
  "csp_report",
  "media",
  "websocket",
  "webtransport",
  "webbundle",
  "other",
];

/**
 * 要件 §5 R1 (b) が「遮断してはならない」と明記した 4 系統の代表 URL。
 *
 * heartbeat はライブの配信状態ポーリング、att/* は bot 判定通過（この方向転換の前提そのもの）、
 * playback / watchtime は視聴履歴の記録経路（維持すると決定済み）。
 * いずれも「計測に見えるが機能に紐づく」ため、テレメトリ一括遮断で最も巻き添えを食いやすい。
 */
const MUST_NOT_BLOCK_URLS = [
  "https://www.youtube.com/youtubei/v1/player/heartbeat?prettyPrint=false",
  "https://www.youtube.com/youtubei/v1/att/get?prettyPrint=false",
  "https://www.youtube.com/youtubei/v1/att/log?prettyPrint=false",
  "https://www.youtube.com/api/stats/playback?ns=yt&el=detailpage&cpn=abc",
  "https://s.youtube.com/api/stats/playback?ns=yt&el=detailpage&cpn=abc",
  "https://www.youtube.com/api/stats/watchtime?ns=yt&el=detailpage&cpn=abc",
  "https://s.youtube.com/api/stats/watchtime?ns=yt&el=detailpage&cpn=abc",
];

/**
 * 要件 §5 R1 (c) と受け入れ条件（再生・シーク・チャット受信が壊れない）が守る URL。
 * 広告そのものの取得も含む —— 広告の回避は目的ではない（§2）。
 */
const MUST_NOT_BLOCK_PLAYBACK_URLS = [
  "https://rr3---sn-abcdefg.googlevideo.com/videoplayback?expire=1&itag=137",
  "https://www.youtube.com/youtubei/v1/player?prettyPrint=false",
  "https://www.youtube.com/youtubei/v1/get_watch?prettyPrint=false",
  "https://www.youtube.com/youtubei/v1/live_chat/get_live_chat?prettyPrint=false",
  "https://www.youtube.com/api/timedtext?v=dQw4w9WgXcQ&lang=ja",
  "https://googleads.g.doubleclick.net/pagead/ads?client=ca-pub-1&ad_type=video",
  "https://www.youtube.com/get_midroll_info?video_id=dQw4w9WgXcQ",
];

/** シークホバーのプレビュー（ストーリーボード）。サムネイル遮断の巻き添えになってはならない。 */
const STORYBOARD_URLS = [
  "https://i.ytimg.com/sb/dQw4w9WgXcQ/storyboard3_L2/M0.jpg?sqp=-oaymwE&sigh=rs",
  "https://i9.ytimg.com/sb/dQw4w9WgXcQ/storyboard3_L1/default.jpg?sqp=-oaymwE",
];

describe("既定で有効な ruleset は (b) を遮断しない", () => {
  for (const url of MUST_NOT_BLOCK_URLS) {
    test(url, () => {
      expect(matchesOf(enabledRules(), url)).toEqual([]);
    });
  }
});

describe("既定で有効な ruleset は再生・チャット・広告配信を遮断しない", () => {
  for (const url of MUST_NOT_BLOCK_PLAYBACK_URLS) {
    test(url, () => {
      expect(matchesOf(enabledRules(), url)).toEqual([]);
    });
  }
});

describe("(a) 純粋な計測系を遮断する", () => {
  const targets = [
    "https://www.youtube.com/youtubei/v1/log_event?alt=json&key=AIza",
    "https://s.youtube.com/api/stats/qoe?event=streamingstats&cpn=abc",
    "https://www.youtube.com/api/stats/qoe?event=streamingstats&cpn=abc",
    "https://www.youtube.com/ptracking?ei=abc&oid=def",
  ];
  for (const url of targets) {
    test(url, () => {
      expect(matchesOf(rulesetById("a-telemetry").rules, url).length).toBeGreaterThan(
        0,
      );
    });
  }
});

describe("(a) 広告トラッキングを遮断する", () => {
  const targets = [
    "https://s.youtube.com/api/stats/ads?ns=yt&ver=2&cpn=abc",
    "https://s.youtube.com/api/stats/atr?ns=yt&ver=2&cpn=abc",
    "https://www.youtube.com/pagead/interaction/?ai=abc&sigh=def",
    "https://googleads.g.doubleclick.net/pagead/interaction/?ai=abc",
    "https://www.youtube.com/pagead/viewthroughconversion/962985656/?random=1",
  ];
  for (const url of targets) {
    test(url, () => {
      expect(
        matchesOf(rulesetById("a-ads-tracking").rules, url).length,
      ).toBeGreaterThan(0);
    });
  }
});

describe("(a) 削除対象領域のサムネイルを遮断する", () => {
  const targets = [
    "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg?sqp=-oaymwE",
    "https://i.ytimg.com/vi_webp/dQw4w9WgXcQ/mqdefault.webp",
    "https://i9.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg",
  ];
  for (const url of targets) {
    test(url, () => {
      expect(
        matchesOf(rulesetById("a-thumbnails").rules, url).length,
      ).toBeGreaterThan(0);
    });
  }
});

describe("サムネイル遮断はストーリーボードを巻き添えにしない", () => {
  for (const url of STORYBOARD_URLS) {
    test(url, () => {
      expect(matchesOf(enabledRules(), url)).toEqual([]);
    });
  }
});

describe("検証専用 ruleset", () => {
  test("rules/experiments/ 配下は manifest 上で無効になっている", () => {
    const experiments = ruleResources().filter((resource) =>
      resource.path.startsWith("rules/experiments/"),
    );
    expect(experiments.length).toBeGreaterThan(0);
    expect(experiments.filter((resource) => resource.enabled)).toEqual([]);
  });

  test("exp-heartbeat は heartbeat を対象にしている", () => {
    expect(
      matchesOf(
        rulesetById("exp-heartbeat").rules,
        "https://www.youtube.com/youtubei/v1/player/heartbeat?prettyPrint=false",
      ).length,
    ).toBeGreaterThan(0);
  });

  test("exp-attestation は att/* を対象にしている", () => {
    expect(
      matchesOf(
        rulesetById("exp-attestation").rules,
        "https://www.youtube.com/youtubei/v1/att/get?prettyPrint=false",
      ).length,
    ).toBeGreaterThan(0);
  });

  test("exp-prefetch は本再生の videoplayback を対象にしない", () => {
    expect(
      matchesOf(
        rulesetById("exp-prefetch").rules,
        "https://rr3---sn-abcdefg.googlevideo.com/videoplayback?expire=1&itag=137",
      ),
    ).toEqual([]);
  });
});

describe("DNR のスキーマとして妥当である", () => {
  const rulesets = loadStaticRulesets();
  const allRules: readonly DnrRule[] = rulesets.flatMap((ruleset) => ruleset.rules);

  test("すべての ruleset が空でないルール配列である", () => {
    for (const ruleset of rulesets) {
      expect(Array.isArray(ruleset.rules)).toBe(true);
      expect(ruleset.rules.length).toBeGreaterThan(0);
    }
  });

  test("id は 1 以上の整数である", () => {
    for (const rule of allRules) {
      expect(Number.isInteger(rule.id)).toBe(true);
      expect(rule.id).toBeGreaterThanOrEqual(1);
    }
  });

  // DNR の要求は ruleset 内での一意性だが、ここでは全 ruleset 横断で一意にする。
  // §9-5 / §9-7 の切り分けでログに出た id から該当ルールを一意に引けるようにするため。
  test("id は全 ruleset 横断で一意である", () => {
    const ids = allRules.map((rule) => rule.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  test("priority は 1 以上の整数で明示されている", () => {
    for (const rule of allRules) {
      expect(Number.isInteger(rule.priority)).toBe(true);
      expect(rule.priority).toBeGreaterThanOrEqual(1);
    }
  });

  test("action.type は DNR が受け付ける値である", () => {
    for (const rule of allRules) {
      // String() で包むのは、未指定（undefined）も一覧に無い値として同じ経路で落とすため。
      expect(ACTION_TYPES).toContain(String(rule.action?.type));
    }
  });

  test("condition は urlFilter を持つ", () => {
    for (const rule of allRules) {
      expect(typeof rule.condition?.urlFilter).toBe("string");
      expect(rule.condition?.urlFilter).not.toBe("");
    }
  });

  /**
   * ドメインアンカー `||` はホスト名の途中で終わると、同名で始まる別ホストにも一致する
   * （`||youtube.com` が `www.youtube.com.example` に一致する。リファレンスが
   * "incorrectly matches" と明言している既知の罠）。
   *
   * 現状のルールは全てホスト直後にパスを持つので踏んでいないが、それは偶然でしかない。
   * 意図より広いホスト集合を掴むルールが (b)/(c) の保護検査をすり抜けて入るのを防ぐため、
   * 「ホスト部の直後で終わらない」ことを構文レベルで強制する。
   */
  test("ドメインアンカーの urlFilter はホスト部の直後で終わらない", () => {
    for (const rule of allRules) {
      const urlFilter = rule.condition?.urlFilter ?? "";
      if (!urlFilter.startsWith("||")) continue;
      expect(urlFilter.slice(2)).toContain("/");
    }
  });

  // regexFilter は url-filter.ts の模擬が扱えない。混ざると (b) の保護検査が
  // 黙って素通りするので、書けないことをテストで固定する。
  test("regexFilter は使わない", () => {
    for (const rule of allRules) {
      expect(rule.condition?.regexFilter).toBeUndefined();
    }
  });

  // 省略時の既定に頼ると、どの種類のリクエストを狙ったのかがルールから読めなくなる。
  test("resourceTypes は妥当な値で明示されている", () => {
    for (const rule of allRules) {
      const resourceTypes = rule.condition?.resourceTypes;
      expect(Array.isArray(resourceTypes)).toBe(true);
      for (const resourceType of resourceTypes ?? []) {
        expect(RESOURCE_TYPES).toContain(resourceType);
      }
    }
  });
});

describe("静的ルールの上限に触れない", () => {
  test("ルール総数が保証下限を超えない", () => {
    const total = loadStaticRulesets().reduce(
      (sum, ruleset) => sum + ruleset.rules.length,
      0,
    );
    expect(total).toBeLessThanOrEqual(GUARANTEED_MINIMUM_STATIC_RULES);
  });

  test("ruleset 数が上限を超えない", () => {
    expect(ruleResources().length).toBeLessThanOrEqual(
      MAX_NUMBER_OF_STATIC_RULESETS,
    );
  });

  test("有効な ruleset 数が上限を超えない", () => {
    expect(
      ruleResources().filter((resource) => resource.enabled).length,
    ).toBeLessThanOrEqual(MAX_NUMBER_OF_ENABLED_STATIC_RULESETS);
  });
});

type ContentScript = {
  readonly matches?: readonly string[];
  readonly js?: readonly string[];
  readonly run_at?: string;
  readonly all_frames?: boolean;
  readonly world?: string;
};

describe("manifest の既存宣言を壊さない", () => {
  test("declarativeNetRequest 権限を宣言している", () => {
    expect(manifestJson.permissions).toContain("declarativeNetRequest");
  });

  /**
   * このタスクは manifest を共有する他タスクと並行して動く。追記のはずの編集が
   * MAIN world の main.js 宣言を書き換えていないことを、ここで機械的に押さえる。
   *
   * content_scripts の件数では検査しない。件数は他タスクが別 world の宣言を正当に
   * 追記しただけで変わるため、守りたい宣言が無傷でも落ちる（過剰仕様）。
   * 守る対象は「main.js を読む宣言が 1 つあり、その中身が意図どおりであること」に限る。
   */
  test("MAIN world の main.js 宣言が無傷である", () => {
    const contentScripts = manifestJson.content_scripts as
      | readonly ContentScript[]
      | undefined;

    // 宣言の同一性は js の中身で引く。world や run_at が壊された場合も、
    // 「見つからない」ではなく「中身が違う」として下の検査で原因が読めるようにするため。
    const declarations = (contentScripts ?? []).filter((script) =>
      script.js?.includes("main.js"),
    );
    // 2 つ以上あれば main.js が二重に読み込まれる。どちらが本体かも決められない。
    expect(declarations.length).toBe(1);

    const declaration = declarations[0];
    expect(declaration?.js).toEqual(["main.js"]);
    expect(declaration?.world).toBe("MAIN");
    expect(declaration?.matches).toEqual([
      "https://www.youtube.com/watch*",
      "https://www.youtube.com/live_chat*",
    ]);
    expect(declaration?.run_at).toBe("document_start");
    expect(declaration?.all_frames).toBe(true);
  });
});
