import { describe, expect, test } from "bun:test";

import { blockPolicyOf, transformTargetOf } from "../src/shared/endpoints";

/** 要件 R1 (a): 純粋な計測で、遮断してよい系統。 */
const BLOCKABLE_URLS = [
  "https://www.youtube.com/youtubei/v1/log_event?prettyPrint=false",
  "https://www.youtube.com/api/stats/qoe?event=streamingstats",
  "https://www.youtube.com/api/stats/ads?ver=2",
  "https://www.youtube.com/api/stats/atr?ns=yt&el=detailpage",
  "https://www.youtube.com/ptracking?video_id=dQw4w9WgXcQ",
  "https://googleads.g.doubleclick.net/pagead/interaction/?ai=CxYZ",
  "https://www.youtube.com/pagead/viewthroughconversion/962985656/?random=1",
];

/** 要件 R1 (b): 機能・認証・視聴履歴に紐づき、遮断してはならない 4 系統。 */
const PROTECTED_URLS = [
  "https://www.youtube.com/youtubei/v1/player/heartbeat?prettyPrint=false",
  "https://www.youtube.com/youtubei/v1/att/get?prettyPrint=false",
  "https://www.youtube.com/api/stats/playback?ns=yt&el=detailpage",
  "https://www.youtube.com/api/stats/watchtime?ns=yt&el=detailpage",
];

describe("blockPolicyOf", () => {
  for (const url of BLOCKABLE_URLS) {
    test(`計測のみの ${url} を blockable と判定する`, () => {
      expect(blockPolicyOf(url)).toBe("blockable");
    });
  }

  for (const url of PROTECTED_URLS) {
    test(`機能に紐づく ${url} を protected と判定する`, () => {
      expect(blockPolicyOf(url)).toBe("protected");
    });
  }

  test("メディアセグメントは unrestricted と判定する", () => {
    expect(
      blockPolicyOf("https://rr3---sn-x.googlevideo.com/videoplayback?itag=140"),
    ).toBe("unrestricted");
  });

  test("watch ページのデータ取得は unrestricted と判定する", () => {
    expect(
      blockPolicyOf("https://www.youtube.com/youtubei/v1/get_watch?prettyPrint=false"),
    ).toBe("unrestricted");
  });

  test("URL として解釈できない文字列は unrestricted と判定する", () => {
    expect(blockPolicyOf("http://[")).toBe("unrestricted");
  });
});

describe("transformTargetOf", () => {
  test("get_watch を watch と判定する", () => {
    expect(
      transformTargetOf("https://www.youtube.com/youtubei/v1/get_watch?prettyPrint=false"),
    ).toBe("watch");
  });

  test("get_live_chat を live_chat と判定する", () => {
    expect(
      transformTargetOf(
        "https://www.youtube.com/youtubei/v1/live_chat/get_live_chat?prettyPrint=false",
      ),
    ).toBe("live_chat");
  });

  test("get_live_chat_replay を live_chat と判定する", () => {
    expect(
      transformTargetOf(
        "https://www.youtube.com/youtubei/v1/live_chat/get_live_chat_replay?prettyPrint=false",
      ),
    ).toBe("live_chat");
  });

  test("相対 URL でも判定できる", () => {
    expect(transformTargetOf("/youtubei/v1/get_watch?prettyPrint=false")).toBe("watch");
  });

  test("メディアセグメントは変換対象と判定しない", () => {
    expect(
      transformTargetOf("https://rr3---sn-x.googlevideo.com/videoplayback?itag=140"),
    ).toBeUndefined();
  });

  test("URL として解釈できない文字列は変換対象と判定しない", () => {
    expect(transformTargetOf("http://[")).toBeUndefined();
  });
});

// 要件 R1 (b) は「遮断してはならない」と明記された系統。将来の誤追加で
// 遮断対象・変換対象に落ちることを機械的に防ぐため、分類の否定を固定する。
describe("要件 R1 (b) の系統に介入しない", () => {
  for (const url of PROTECTED_URLS) {
    test(`${url} を遮断対象に分類しない`, () => {
      expect(blockPolicyOf(url)).not.toBe("blockable");
    });

    test(`${url} を変換対象に分類しない`, () => {
      expect(transformTargetOf(url)).toBeUndefined();
    });
  }
});
