import { readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, test } from "bun:test";

import {
  CHAT_CLOSED_LAYOUT_SELECTOR,
  CHAT_COLUMN_SELECTOR,
  CHAT_DISPLAY_CSS,
  CHAT_DISPLAY_STYLE_ID,
  CHAT_EMPTY_AVATAR_SELECTOR,
  CHAT_LAYOUT_SELECTOR,
  CHAT_MESSAGE_SELECTOR,
  PLAYER_COLUMN_SELECTOR,
  applyChatDisplay,
  applyPanelWidth,
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
 * 出所: 2026-08-02 に実ブラウザ（未ログイン）の配信中のライブで確認した DOM。パネル幅は watch
 * ページのレイアウト要素（`ytd-watch-flexy`）、文字サイズとアイコンの枠はライブチャットの iframe
 * （`/live_chat`）の中で、当てる文書が違う。実 DOM に当たるかどうかは実ブラウザでしか確かめられ
 * ないので、ここでは確認した文字列を固定して、書き換えるなら再確認が要ることを示す。
 */
describe("適用先のセレクタ", () => {
  /**
   * 幅は列を名指しせず、YouTube 自身が幅に使う変数を差し替える。名指しできる列が版によって
   * 変わる（`#secondary` が列ではなく画面幅いっぱいの入れ物である版がある）ため。
   */
  test("パネル幅はライブチャットのある watch ページのレイアウトを名指しする", () => {
    expect(CHAT_LAYOUT_SELECTOR).toBe("ytd-watch-flexy:has(ytd-live-chat-frame)");
  });

  test("パネル幅は YouTube 自身の列幅の変数を差し替える", () => {
    expect(CHAT_DISPLAY_CSS).toContain("--ytd-watch-flexy-sidebar-width:");
  });

  // 列に幅を直接指定すると、列でない要素を掴んだ版でレイアウトが崩れる。指定しないことを固定する。
  test("パネル幅は列の幅を直接指定しない", () => {
    expect(CHAT_DISPLAY_CSS).not.toMatch(/^\s*width:/m);
  });

  test("幅の下限は列になり得る 2 つの要素から外す", () => {
    expect(CHAT_COLUMN_SELECTOR).toBe(
      "#secondary:has(ytd-live-chat-frame), #secondary-inner:has(ytd-live-chat-frame)",
    );
  });

  /**
   * プレーヤーの列の下限も外す。外さないと 2 カラム表示でチャットが指定の比率へ届かず
   * （実測: 幅 1061px の窓で 0.45 の指定が 358px で止まる）、「シアター表示のときだけ
   * 幅が変わる」ように見える。
   */
  test("幅の下限はプレーヤーの列からも外す", () => {
    expect(PLAYER_COLUMN_SELECTOR).toBe(
      "ytd-watch-flexy:has(ytd-live-chat-frame) #primary",
    );

    const rule = CHAT_DISPLAY_CSS.slice(CHAT_DISPLAY_CSS.indexOf(PLAYER_COLUMN_SELECTOR));
    expect(rule).toContain("min-width: 0 !important");
  });

  /**
   * チャットを閉じている間は列に幅を持たせない。持たせたままだと、「次の動画」を消してある本拡張
   * では空白だけが残り、プレーヤーも広がらない。開いているパネルがあるときは畳まない。
   */
  test("閉じているチャットの列は幅を 0 にする", () => {
    expect(CHAT_CLOSED_LAYOUT_SELECTOR).toBe(
      'ytd-watch-flexy:has(ytd-live-chat-frame[collapsed]):not(:has(#secondary-inner ytd-engagement-panel-section-list-renderer[visibility="ENGAGEMENT_PANEL_VISIBILITY_EXPANDED"]))',
    );

    const rule = CHAT_DISPLAY_CSS.slice(
      CHAT_DISPLAY_CSS.indexOf(CHAT_CLOSED_LAYOUT_SELECTOR),
    );
    expect(rule).toContain("--ytd-watch-flexy-sidebar-width: 0px !important");
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
    expect(CHAT_DISPLAY_CSS).toContain(CHAT_LAYOUT_SELECTOR);
    expect(CHAT_DISPLAY_CSS).toContain(CHAT_COLUMN_SELECTOR);
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

/**
 * ドラッグ中は幅だけを当て直す。文字サイズを持ち回らずに幅を動かせること、当てる値が
 * 設定の範囲を出ないことを、掴む側（chat-resize）から見た入口として固定する。
 */
describe("applyPanelWidth", () => {
  const panelWidth = (variables: Map<string, string>): string | undefined =>
    variables.get("--youtube-super-lite-chat-panel-width");

  test("幅の比率を CSS カスタムプロパティとして与える", () => {
    const { host, variables } = fakeHost();

    applyPanelWidth(0.4, host);

    expect(panelWidth(variables)).toBe("calc(0.4 * 100vw)");
  });

  test("文字サイズには触らない", () => {
    const { host, variables } = fakeHost();
    applyChatDisplay({ fontSizePx: 22, panelWidthRatio: 0.2 }, host);

    applyPanelWidth(0.4, host);

    expect(variables.get("--youtube-super-lite-chat-font-size")).toBe("22px");
  });

  test("範囲を超えた比率は範囲内へ収める", () => {
    const { host, variables } = fakeHost();

    applyPanelWidth(0.9, host);

    expect(panelWidth(variables)).toBe("calc(0.6 * 100vw)");
  });

  test("差し込み先が無くても例外を投げず、効かなかったことを残す", () => {
    const { host } = fakeHost({ rootMissing: true });

    const { messages } = captureDebug(() => {
      applyPanelWidth(0.4, host);
    });

    expect(messages.length).toBe(1);
  });
});

describe("サイドパネルのチャット表示 UI", () => {
  const html = readFileSync(
    path.join(import.meta.dir, "../src/side-panel/side-panel.html"),
    "utf8",
  );

  const chatDisplaySection = (): string =>
    html.slice(
      html.indexOf('<section id="chat-display">'),
      html.indexOf("</section>", html.indexOf('<section id="chat-display">')),
    );

  /**
   * スライダーは HTML 側に置き、範囲と現在値だけをスクリプトが与える。
   * 対応する要素が無ければスクリプトは動かないので、id の対応をここで固定する。
   */
  test("チャット表示の区画に文字サイズのスライダーを持つ", () => {
    expect(chatDisplaySection()).toContain('id="chat-font-size"');
  });

  /**
   * 幅のつまみはサイドパネルに置かない（ユーザー決定 2026-08-02）。操作の場所は watch ページの
   * ハンドル 1 つに絞る。両方に置くと、どちらが今の値かを合わせ続ける配線が要る。
   */
  test("チャット表示の区画に幅のスライダーを持たない", () => {
    expect(chatDisplaySection()).not.toContain('id="chat-panel-width"');
  });
});

describe("startChatDisplay", () => {
  const start = (stored: Record<string, unknown> = {}) => {
    const { store } = fakeStore(stored);
    const dom = fakeHost();
    // 登録時には呼ばない。document_start では onNavigated が初回を DOMContentLoaded まで
    // 遅らせるので、そこを待たずに当てることを起動時の適用として検査するため。
    const navigated: (() => void)[] = [];
    // 幅が変わったことをページへ知らせたかを覗く。実体は window。
    const notified: string[] = [];
    startChatDisplay({
      store,
      host: dom.host,
      view: {
        dispatchEvent: (event: Event) => {
          notified.push(event.type);
          return true;
        },
      },
      navigate: (apply) => {
        navigated.push(apply);
      },
    });
    return {
      store,
      dom,
      notified,
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

  /**
   * 幅を当てただけではプレーヤーの中の映像は前の大きさのままなので、当てたことを知らせる。
   * 保存値が届いた初回にも要る（保存された幅で開いた watch ページがはみ出したままになる）。
   */
  test("当てたらプレーヤーへ測り直させる", async () => {
    const { notified } = start({ chatDisplay: { fontSizePx: 22, panelWidthRatio: 0.4 } });

    await flush();

    expect(notified).toEqual(["resize"]);
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
