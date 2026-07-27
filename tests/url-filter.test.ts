import { describe, expect, test } from "bun:test";

import { matchesUrlFilter } from "./support/url-filter";

// 期待値は declarativeNetRequest リファレンスの urlFilter 節に載っている例をそのまま使う。
// https://developer.chrome.com/docs/extensions/reference/api/declarativeNetRequest
// 自前実装なので、仕様の出所を持たない期待値を足すと「実装がそう動くから正しい」に転ぶ。
describe("matchesUrlFilter", () => {
  describe("ワイルドカード", () => {
    test("* は任意個の文字に一致する", () => {
      expect(matchesUrlFilter("abc*d", "https://abcd.com")).toBe(true);
      expect(matchesUrlFilter("abc*d", "https://example.com/abcxyzd")).toBe(
        true,
      );
    });
  });

  describe("セパレータ ^", () => {
    test("英数字・_-.% 以外の 1 文字に一致する", () => {
      expect(matchesUrlFilter("example*^123|", "https://example.com/123")).toBe(
        true,
      );
    });

    test("英数字が続く位置には一致しない", () => {
      expect(matchesUrlFilter("example*^123|", "https://example.com/1234")).toBe(
        false,
      );
    });

    test("URL の終端にも一致する", () => {
      expect(matchesUrlFilter("example.com/123^", "https://example.com/123")).toBe(
        true,
      );
    });
  });

  describe("ドメインアンカー ||", () => {
    test("サブドメインの先頭にも一致する", () => {
      expect(matchesUrlFilter("||a.example.com", "https://a.example.com/")).toBe(
        true,
      );
      expect(
        matchesUrlFilter("||a.example.com", "https://b.a.example.com/xyz"),
      ).toBe(true);
    });

    test("ドメインの途中では一致しない", () => {
      expect(matchesUrlFilter("||a.example.com", "https://example.com/")).toBe(
        false,
      );
    });

    test("クエリ文字列に現れたホスト名では一致しない", () => {
      expect(
        matchesUrlFilter("||google.com/", "https://example.com/?param=google.com/"),
      ).toBe(false);
    });

    // リファレンスが "incorrectly matches" と明言している既知の罠。仕様どおりの挙動なので
    // 模擬側でも再現する。ルール側でこれを踏まないことは rules.test.ts が強制する。
    test("末尾に / が無いと同名で始まる別ホストにも一致してしまう", () => {
      expect(
        matchesUrlFilter("||youtube.com", "https://www.youtube.com.example/foo"),
      ).toBe(true);
      expect(
        matchesUrlFilter("||youtube.com/", "https://www.youtube.com.example/foo"),
      ).toBe(false);
    });
  });

  describe("左右アンカー |", () => {
    test("先頭の | は URL の先頭に固定する", () => {
      expect(matchesUrlFilter("|https*", "https://example.com")).toBe(true);
      expect(matchesUrlFilter("|https*", "http://example.com/")).toBe(false);
    });

    test("末尾の | は URL の末尾に固定する", () => {
      expect(matchesUrlFilter("example.com/a|", "https://example.com/a")).toBe(
        true,
      );
      expect(matchesUrlFilter("example.com/a|", "https://example.com/ab")).toBe(
        false,
      );
    });
  });

  describe("大文字小文字", () => {
    // isUrlFilterCaseSensitive の既定は false。既定で照合する模擬なので区別しない。
    test("既定では区別しない", () => {
      expect(matchesUrlFilter("||youtube.com/API/", "https://www.youtube.com/api/")).toBe(
        true,
      );
    });
  });

  describe("正規表現のメタ文字", () => {
    test(". は文字どおりに扱う", () => {
      expect(matchesUrlFilter("||youtube.com/", "https://youtubeXcom/")).toBe(
        false,
      );
    });

    test("? は文字どおりに扱う", () => {
      expect(matchesUrlFilter("/ptracking?", "https://www.youtube.com/ptracking?ei=1")).toBe(
        true,
      );
      expect(matchesUrlFilter("/ptracking?", "https://www.youtube.com/ptracking")).toBe(
        false,
      );
    });
  });
});
