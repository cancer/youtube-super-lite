import { readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, test } from "bun:test";

import {
  CHAT_DISPLAY_CSS,
  CHAT_DISPLAY_STYLE_ID,
  CHAT_EMPTY_AVATAR_SELECTOR,
  CHAT_MESSAGE_SELECTOR,
  CHAT_PANEL_SELECTOR,
  applyChatDisplay,
  chatDisplayVariables,
  startChatDisplay,
  type StyleHost,
} from "../src/isolated/chat-display";
import { chatDisplaySection, writeSection } from "../src/shared/settings";

import { flush } from "./support/flush";
import { fakeStore } from "./support/settings-store";

/**
 * document のフェイク。
 *
 * 実際に効いたかどうか（CSS カスタムプロパティの値・スタイルの差し込み）を覗ける形にしてある。
 * 差し込み先を捨てたり差し込みを失敗させたりできるのは、YouTube 側の DOM が想定と違っても
 * 視聴を止めないことを検証するため。
 */
const fakeHost = (
  options: { readonly rootMissing?: boolean; readonly insertFails?: boolean } = {},
): {
  host: StyleHost;
  variables: Map<string, string>;
  styles: { id: string; textContent: string | null }[];
  detachStyles: () => void;
} => {
  const variables = new Map<string, string>();
  const styles: { id: string; textContent: string | null }[] = [];
  const root = {
    style: {
      setProperty: (property: string, value: string): void => {
        variables.set(property, value);
      },
    },
    insertAdjacentElement: (
      _position: "beforeend",
      element: { id: string; textContent: string | null },
    ): unknown => {
      if (options.insertFails === true) throw new Error("差し込めない DOM");
      styles.push(element);
      return element;
    },
  };
  return {
    host: {
      documentElement: options.rootMissing === true ? null : root,
      createElement: () => ({ id: "", textContent: null }),
      getElementById: (elementId) =>
        styles.find((style) => style.id === elementId) ?? null,
    },
    variables,
    styles,
    detachStyles: () => {
      styles.length = 0;
    },
  };
};

/** console.debug の呼び出しを捕まえる。効かなかったことが外から分かるかを検証するため。 */
const captureDebug = <T>(run: () => T): { result: T; messages: unknown[][] } => {
  const messages: unknown[][] = [];
  const original = console.debug;
  console.debug = (...args: unknown[]): void => {
    messages.push(args);
  };
  try {
    return { result: run(), messages };
  } finally {
    console.debug = original;
  }
};

describe("chatDisplayVariables", () => {
  test("既定値を 16px / 0.28 の CSS 値にする", () => {
    expect(chatDisplayVariables(chatDisplaySection.defaults)).toEqual({
      "--youtube-super-lite-chat-font-size": "16px",
      "--youtube-super-lite-chat-panel-width": "calc(0.28 * 100vw)",
    });
  });

  test("下限の設定値を CSS 値にする", () => {
    expect(
      chatDisplayVariables({ fontSizePx: 10, panelWidthRatio: 0.15 }),
    ).toEqual({
      "--youtube-super-lite-chat-font-size": "10px",
      "--youtube-super-lite-chat-panel-width": "calc(0.15 * 100vw)",
    });
  });

  test("上限の設定値を CSS 値にする", () => {
    expect(chatDisplayVariables({ fontSizePx: 28, panelWidthRatio: 0.6 })).toEqual({
      "--youtube-super-lite-chat-font-size": "28px",
      "--youtube-super-lite-chat-panel-width": "calc(0.6 * 100vw)",
    });
  });

  // 型の外から来た値がここまで届き得るので、CSS へ渡す直前でも範囲を保証する。
  test("範囲を超えた値は範囲内へ収めてから CSS 値にする", () => {
    expect(chatDisplayVariables({ fontSizePx: 400, panelWidthRatio: 0.9 })).toEqual({
      "--youtube-super-lite-chat-font-size": "28px",
      "--youtube-super-lite-chat-panel-width": "calc(0.6 * 100vw)",
    });
  });

  test("範囲に満たない値は範囲内へ収めてから CSS 値にする", () => {
    expect(chatDisplayVariables({ fontSizePx: 1, panelWidthRatio: 0.01 })).toEqual({
      "--youtube-super-lite-chat-font-size": "10px",
      "--youtube-super-lite-chat-panel-width": "calc(0.15 * 100vw)",
    });
  });
});

/**
 * 適用先のセレクタ。
 *
 * 出所: 2026-08-01 に実ブラウザ（未ログイン）の配信中のライブの watch ページで確認した DOM。
 * パネル幅は watch ページの右の列（`#secondary`）、文字サイズはライブチャットの iframe
 * （`/live_chat`）の中の発言の要素で、当てる文書が違う。実 DOM に当たるかどうかは実ブラウザで
 * しか確かめられないので、ここでは確認した文字列を固定して、書き換えるなら再確認が要ることを示す。
 */
describe("適用先のセレクタ", () => {
  test("パネル幅はライブチャットが入っている列だけを名指しする", () => {
    expect(CHAT_PANEL_SELECTOR).toBe("#secondary:has(ytd-live-chat-frame)");
  });

  test("文字サイズは発言の一覧の直下の子を名指しする", () => {
    expect(CHAT_MESSAGE_SELECTOR).toBe(
      "yt-live-chat-item-list-renderer #items > *",
    );
  });

  /**
   * アイコンの枠は「画像が入っていない」ことだけで選ぶ。誰のアイコンを残すかは R3（chat-images）の
   * 判定が持つので、こちらへ書き写さない。`data:` を除くのは、いちど画像を載せた枠を作り直したとき
   * 1x1 の透明 GIF が入るため（どちらも中身が無い）。
   */
  test("空のアイコンの枠は画像 URL の有無だけで選ぶ", () => {
    expect(CHAT_EMPTY_AVATAR_SELECTOR).toBe(
      'yt-live-chat-renderer #author-photo:not(:has(img[src]:not([src^="data:"])))',
    );
  });

  test("空のアイコンの枠は投稿者の種類を見ない", () => {
    expect(CHAT_EMPTY_AVATAR_SELECTOR).not.toContain("author-type");
  });

  test("規則はどのセレクタも持つ", () => {
    expect(CHAT_DISPLAY_CSS).toContain(CHAT_PANEL_SELECTOR);
    expect(CHAT_DISPLAY_CSS).toContain(CHAT_MESSAGE_SELECTOR);
    expect(CHAT_DISPLAY_CSS).toContain(CHAT_EMPTY_AVATAR_SELECTOR);
  });

  // 空白を詰めるには枠を畳む（display: none）必要がある。透明にするだけでは場所が残る。
  test("空のアイコンの枠は畳んで場所を残さない", () => {
    const rule = CHAT_DISPLAY_CSS.slice(
      CHAT_DISPLAY_CSS.indexOf(CHAT_EMPTY_AVATAR_SELECTOR),
    );

    expect(rule).toContain("display: none !important");
  });
});

describe("CHAT_DISPLAY_CSS", () => {
  /**
   * 変数名の綴り違いは CSS では黙って無視され、指定が丸ごと効かなくなる。
   * 規則が使う変数と与える変数の対応をここで固定する。
   */
  test("使う変数はすべて chatDisplayVariables が与える", () => {
    const used = new Set(
      [...CHAT_DISPLAY_CSS.matchAll(/var\((--[\w-]+)\)/g)].map((match) => match[1]),
    );

    expect([...used].sort()).toEqual(
      Object.keys(chatDisplayVariables(chatDisplaySection.defaults)).sort(),
    );
  });
});

describe("applyChatDisplay", () => {
  test("設定値を CSS カスタムプロパティとして与える", () => {
    const { host, variables } = fakeHost();

    applyChatDisplay({ fontSizePx: 22, panelWidthRatio: 0.4 }, host);

    expect(Object.fromEntries(variables)).toEqual({
      "--youtube-super-lite-chat-font-size": "22px",
      "--youtube-super-lite-chat-panel-width": "calc(0.4 * 100vw)",
    });
  });

  test("規則を持つスタイルを差し込む", () => {
    const { host, styles } = fakeHost();

    applyChatDisplay(chatDisplaySection.defaults, host);

    expect(styles).toEqual([
      { id: CHAT_DISPLAY_STYLE_ID, textContent: CHAT_DISPLAY_CSS },
    ]);
  });

  // 遷移ごとに呼ばれる。同じ規則が積み上がってはいけない。
  test("繰り返し呼んでもスタイルは 1 つだけ", () => {
    const { host, styles } = fakeHost();

    applyChatDisplay(chatDisplaySection.defaults, host);
    applyChatDisplay(chatDisplaySection.defaults, host);
    applyChatDisplay(chatDisplaySection.defaults, host);

    expect(styles.length).toBe(1);
  });

  test("繰り返し呼んだら後の設定値で上書きする", () => {
    const { host, variables } = fakeHost();

    applyChatDisplay({ fontSizePx: 12, panelWidthRatio: 0.2 }, host);
    applyChatDisplay({ fontSizePx: 24, panelWidthRatio: 0.5 }, host);

    expect(variables.get("--youtube-super-lite-chat-font-size")).toBe("24px");
  });

  // YouTube 側の描画でスタイルごと消えることがあり得る。次の適用で戻せること。
  test("差し込んだスタイルが消えていたら入れ直す", () => {
    const { host, styles, detachStyles } = fakeHost();
    applyChatDisplay(chatDisplaySection.defaults, host);

    detachStyles();
    applyChatDisplay(chatDisplaySection.defaults, host);

    expect(styles.length).toBe(1);
  });

  test("差し込み先が無くても例外を投げず、効かなかったことを残す", () => {
    const { host } = fakeHost({ rootMissing: true });

    const { messages } = captureDebug(() => {
      applyChatDisplay(chatDisplaySection.defaults, host);
    });

    expect(messages.length).toBe(1);
  });

  test("DOM の操作が失敗しても例外を投げず、効かなかったことを残す", () => {
    const { host } = fakeHost({ insertFails: true });

    const { messages } = captureDebug(() => {
      applyChatDisplay(chatDisplaySection.defaults, host);
    });

    expect(messages.length).toBe(1);
  });
});

describe("サイドパネルのチャット表示 UI", () => {
  const html = readFileSync(
    path.join(import.meta.dir, "../src/side-panel/side-panel.html"),
    "utf8",
  );

  /**
   * スライダーは HTML 側に置き、範囲と現在値だけをスクリプトが与える。
   * 対応する要素が無ければスクリプトは動かないので、id の対応をここで固定する。
   */
  test("チャット表示の区画に 2 つのスライダーを持つ", () => {
    const section = html.slice(
      html.indexOf('<section id="chat-display">'),
      html.indexOf("</section>", html.indexOf('<section id="chat-display">')),
    );

    expect(section).toContain('id="chat-font-size"');
    expect(section).toContain('id="chat-panel-width"');
  });
});

describe("startChatDisplay", () => {
  const start = (stored: Record<string, unknown> = {}) => {
    const { store } = fakeStore(stored);
    const dom = fakeHost();
    // 登録時には呼ばない。document_start では onNavigated が初回を DOMContentLoaded まで
    // 遅らせるので、そこを待たずに当てることを起動時の適用として検査するため。
    const navigated: (() => void)[] = [];
    startChatDisplay({
      store,
      host: dom.host,
      navigate: (apply) => {
        navigated.push(apply);
      },
    });
    return {
      store,
      dom,
      navigate: () => {
        for (const apply of navigated) apply();
      },
    };
  };

  const fontSize = (variables: Map<string, string>): string | undefined =>
    variables.get("--youtube-super-lite-chat-font-size");

  test("保存値を読んで当てる", async () => {
    const { dom } = start({ chatDisplay: { fontSizePx: 22, panelWidthRatio: 0.4 } });

    await flush();

    expect(fontSize(dom.variables)).toBe("22px");
  });

  test("未保存なら既定値を当てる", async () => {
    const { dom } = start();

    await flush();

    expect(fontSize(dom.variables)).toBe("16px");
  });

  test("設定が変わったら当て直す", async () => {
    const { store, dom } = start();
    await flush();

    await writeSection(store, chatDisplaySection, {
      fontSizePx: 20,
      panelWidthRatio: 0.3,
    });

    expect(fontSize(dom.variables)).toBe("20px");
  });

  // 遷移では文書が作り直されないので、当てた CSS が残っているとは限らない。当て直す。
  test("遷移のたびに当て直す", async () => {
    const { dom, navigate } = start({ chatDisplay: { fontSizePx: 22, panelWidthRatio: 0.4 } });
    await flush();
    dom.detachStyles();

    navigate();
    await flush();

    expect(dom.styles).toHaveLength(1);
  });
});
