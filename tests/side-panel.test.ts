import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, test } from "bun:test";

/**
 * 操作面がサイドパネルとして成立する条件を、宣言と配線の側から固定する。
 *
 * サイドパネルは実際に開かせて確かめられない（拡張を読み込んだ Chrome が要る）。
 * 開くために必要な宣言（manifest）とスクリプトの読み込み経路（HTML）、そして
 * アイコンのクリックに紐づける設定（service worker）が揃っていることを、
 * ここで機械的に押さえる。
 *
 * 検査対象が manifest.json / HTML / service worker に散るのは、「サイドパネルが開く」が
 * 3 つの宣言の一致で初めて成立し、どれか 1 つだけを見ても成否が判定できないため。
 * 他のテストのようにモジュール 1 つへは対応しない。
 *
 * 各項目の根拠は chrome.sidePanel の公式リファレンス。
 * https://developer.chrome.com/docs/extensions/reference/api/sidePanel
 */

const root = path.resolve(import.meta.dir, "..");

const manifest = JSON.parse(
  readFileSync(path.join(root, "manifest.json"), "utf8"),
) as Record<string, unknown>;

type SidePanelKey = { readonly default_path?: string };
type ActionKey = { readonly default_popup?: string };

const sidePanelKey = (): SidePanelKey | undefined =>
  manifest.side_panel as SidePanelKey | undefined;

const readSidePanelHtml = (): string =>
  readFileSync(path.join(root, "src/side-panel/side-panel.html"), "utf8");

describe("manifest がサイドパネルを宣言している", () => {
  test("sidePanel 権限を宣言している", () => {
    expect(manifest.permissions).toContain("sidePanel");
  });

  test("side_panel.default_path でパネルの中身を指している", () => {
    expect(sidePanelKey()?.default_path).toBe("side-panel.html");
  });

  test("default_path が指す HTML がソースにある", () => {
    const defaultPath = sidePanelKey()?.default_path;
    // 未宣言のまま join すると src/side-panel ディレクトリ自体を指してしまい、
    // 「宣言が無い」が「ファイルがある」として通り抜ける。先に型で落とす。
    expect(typeof defaultPath).toBe("string");

    expect(existsSync(path.join(root, "src/side-panel", String(defaultPath)))).toBe(
      true,
    );
  });

  /**
   * openPanelOnActionClick はツールバーのアイコンの挙動を差し替える設定なので、
   * アイコンの宣言（action）自体は残っている必要がある。
   */
  test("action を宣言している", () => {
    expect(manifest.action).toBeDefined();
  });

  test("アイコンのクリック先を popup に戻していない", () => {
    expect((manifest.action as ActionKey | undefined)?.default_popup).toBeUndefined();
  });
});

describe("サイドパネルの HTML", () => {
  // 拡張ページには MV3 の CSP が効くため、インラインの <script> は実行されない。
  test("インラインスクリプトを持たない", () => {
    const inlineScripts =
      readSidePanelHtml().match(/<script(?![^>]*\ssrc\s*=)[^>]*>/gi) ?? [];

    expect(inlineScripts).toEqual([]);
  });

  // src はビルドの出力名（dist 直下）と対応する。default_path と対にしておかないと、
  // パネルは開くのに中身の配線だけが動かない状態になる。
  test("default_path と対になる名前の外部スクリプトだけを読む", () => {
    const sources = [
      ...readSidePanelHtml().matchAll(/<script[^>]*\ssrc\s*=\s*["']([^"']+)["']/gi),
    ].map((match) => match[1]);

    expect(sources).toEqual(["side-panel.js"]);
  });
});

/**
 * 操作 UI の要素は側パネルの配線（side-panel/index.ts）が id で引く。HTML 側から消えると
 * パネルを開いた瞬間に配線が落ちるので、対になる id があることをここで押さえる。
 */
describe("watch ページの整理の操作 UI", () => {
  const checkboxIds = (): string[] =>
    [
      ...readSidePanelHtml().matchAll(/<input\b[^>]*type\s*=\s*["']checkbox["'][^>]*>/gi),
    ].flatMap((tag) => [...tag[0].matchAll(/\bid\s*=\s*["']([^"']+)["']/gi)].map((m) => m[1]));

  test("コメント欄を消すかどうかのチェックボックスがある", () => {
    expect(checkboxIds()).toContain("remove-comments");
  });
});

describe("service worker", () => {
  /**
   * 他のテストのように依存を引数で渡さず chrome ごと差し替えるのは、検査したいものが
   * 「起動しただけで設定が行なわれる」という副作用そのものだから。sw は実物の chrome を
   * 配線する場所（合成点）なので、そこを通さずに配線の有無は確かめられない。
   */
  test("ツールバーのアイコンのクリックでサイドパネルが開くよう設定する", async () => {
    const behaviors: unknown[] = [];
    const globals = globalThis as unknown as { chrome?: unknown };
    const original = globals.chrome;
    globals.chrome = {
      runtime: { onInstalled: { addListener: () => {} } },
      sidePanel: {
        setPanelBehavior: (behavior: unknown): Promise<void> => {
          behaviors.push(behavior);
          return Promise.resolve();
        },
      },
    };

    try {
      await import("../src/background/sw");
    } finally {
      globals.chrome = original;
    }

    expect(behaviors).toEqual([{ openPanelOnActionClick: true }]);
  });
});
