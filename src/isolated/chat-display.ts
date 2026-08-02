import { onNavigated } from "../shared/navigation";
import {
  applySection,
  chatDisplaySection,
  localSettingsStore,
  watchSection,
  type ChatDisplaySettings,
  type SettingsStore,
} from "../shared/settings";

/**
 * R5 のチャット表示（文字サイズ・パネル幅）と、アイコンを落とした跡の空白を CSS で当てる。
 *
 * 当てる先は 2 つの文書にまたがる。パネル幅は watch ページの列で、文字サイズとアイコンの枠は
 * ライブチャットの iframe（`/live_chat`）の中にある。content script は `all_frames` で両方に
 * 注入されるので、規則は 1 枚にまとめて両方へ差し込む。片方でしか一致しない規則は、もう片方では
 * 単に効かない。
 *
 * 値は CSS カスタムプロパティで渡す。設定が変わったときに差し込んだ規則を書き換えず、
 * 変数の値だけを差し替えれば済むため。
 */

/** 差し込むスタイルの id。同じものを二重に入れないための目印であり、再適用の判定にも使う。 */
export const CHAT_DISPLAY_STYLE_ID = "youtube-super-lite-chat-display";

const FONT_SIZE_VARIABLE = "--youtube-super-lite-chat-font-size";
const PANEL_WIDTH_VARIABLE = "--youtube-super-lite-chat-panel-width";

/**
 * 幅を変える列（watch ページ側）。
 *
 * 出所: 2026-08-01 に実ブラウザ（未ログイン）の配信中のライブの watch ページで確認した DOM。
 * `#secondary` はライブチャットと「次の動画」が入る右の列で、幅は YouTube 自身が
 * `width: var(--ytd-watch-flexy-sidebar-width)` として直接指定している。
 *
 * ライブチャットが差し込まれたときだけ幅を変える（`:has()`）。チャットの無い動画で右の列だけが
 * 動くのを避けるため。1 カラム表示ではチャットはこの列の外（動画の下）へ移るので、そもそも
 * 一致せず既定のままになる。
 */
export const CHAT_PANEL_SELECTOR = "#secondary:has(ytd-live-chat-frame)";

/**
 * 中身が入らなかった投稿者アイコンの枠（live_chat の文書側）。
 *
 * R3 が応答から `authorPhoto` を落とすと、枠だけが中身なしで残る。枠は 24px 幅・右 16px の
 * 余白を持つため、アイコンがあった位置に 40px の空白が空く。畳んで詰める。
 * 出所: 2026-08-02 に実ブラウザで、R3 と同じ判定で応答を書き換えて確認した DOM。
 *
 * 「誰のアイコンを出すか」はここでは決めない。見るのは画像が入っているかどうかだけなので、
 * 残す側（モデレーター・チャンネル所有者）の枠はそのままで、落とした側だけが畳まれる。R3 の
 * 判定を二重に持たないためで、表示対象が変わってもこの規則は書き換えずに追随する。
 *
 * 空の見分け方に 2 つ要るのは、枠の作られ方が 2 通りあるため。新しく作られた枠は `<img>` が
 * `src` を持たず、いちど画像を載せた枠を作り直した場合は 1x1 の透明 GIF（`data:` URL）が入る。
 * どちらも「画像 URL が入っていない」なので、`data:` でない `src` が無いことを条件にする。
 */
export const CHAT_EMPTY_AVATAR_SELECTOR =
  'yt-live-chat-renderer #author-photo:not(:has(img[src]:not([src^="data:"])))';

/**
 * 文字サイズを変える要素（live_chat の文書側）。
 *
 * 出所は CHAT_PANEL_SELECTOR と同じ確認。発言 1 件ぶんの要素で、YouTube 自身が
 * `font-size: 13px` を直接指定している。名前・本文はここから継承するので、種類ごとの要素
 * （通常の発言・スーパーチャット・メンバー加入）を数え上げず、一覧の直下の子をまとめて指す。
 * 時刻だけは 11px の直接指定を持つため変わらない。
 */
export const CHAT_MESSAGE_SELECTOR = "yt-live-chat-item-list-renderer #items > *";

/**
 * 当てる規則。
 *
 * どちらも YouTube 自身の直接指定を上書きするので `!important` が要る。
 * `min-width` を外すのは、YouTube の下限（実測 320px）が設定の下限（0.15）より広く、
 * 狭い側の指定がそこで止まってしまうため。列は縮み得るので、プレーヤーの最小幅に阻まれる
 * 比率では指定より狭くなる。
 *
 * 一致しなくなっても効かなくなるだけで、視聴は続けられる。
 */
export const CHAT_DISPLAY_CSS = `${CHAT_PANEL_SELECTOR} {
  width: var(${PANEL_WIDTH_VARIABLE}) !important;
  min-width: 0 !important;
}

${CHAT_MESSAGE_SELECTOR} {
  font-size: var(${FONT_SIZE_VARIABLE}) !important;
}

${CHAT_EMPTY_AVATAR_SELECTOR} {
  display: none !important;
}
`;

/**
 * 設定値を CSS の値にする。
 *
 * `ChatDisplaySettings` の型は「数値である」としか言っておらず範囲を表せないので、範囲の保証は
 * 区画の normalize に任せる（範囲と既定値の定義は shared/settings のものだけ）。今の呼び出し元は
 * storage 経由なので既に正規化済みの値が来るが、その経路を知らなくても CSS へ出す値が範囲内で
 * あることをこの関数だけで言い切れる形にしてある。
 */
export const chatDisplayVariables = (
  settings: ChatDisplaySettings,
): Readonly<Record<string, string>> => {
  const { fontSizePx, panelWidthRatio } = chatDisplaySection.normalize(settings);
  return {
    [FONT_SIZE_VARIABLE]: `${fontSizePx}px`,
    // 比率のまま渡して単位は CSS 側で掛ける。px へ先に直すと窓の大きさが変わったときに追随しない。
    [PANEL_WIDTH_VARIABLE]: `calc(${panelWidthRatio} * 100vw)`,
  };
};

/** スタイルを差し込む先。実体は document で、テストではフェイクを渡す。 */
export type StyleHost = {
  readonly documentElement: StyleHostRoot | null;
  createElement(tagName: "style"): StyleHostElement;
  getElementById(elementId: string): StyleHostElement | null;
};

/**
 * 変数の置き場所とスタイルの差し込み先を兼ねる根の要素（`<html>`）。
 *
 * 末尾への追加に `append` ではなく `insertAdjacentElement` を使うのは、`append` の引数が
 * `Node` を要求し、実 `Document` をこの型へ当てはめられないため（`Node` を満たすフェイクは作れない）。
 * どちらも「末尾に入れる」で同じ意味になる。
 */
type StyleHostRoot = {
  readonly style: { setProperty(property: string, value: string): void };
  insertAdjacentElement(position: "beforeend", element: StyleHostElement): unknown;
};

type StyleHostElement = {
  id: string;
  textContent: string | null;
};

/** 効かなかったことを残す。壊れても視聴は続くので、失敗はここに出すだけにする。 */
const debug = (...message: unknown[]): void => {
  console.debug("[youtube-super-lite] チャット表示:", ...message);
};

/**
 * 設定を今の文書へ当てる。何度呼んでも結果は同じ（SPA 遷移ごとに呼ぶ）。
 *
 * DOM が想定と違っても例外は投げない。チャットの見た目が既定に戻るだけで視聴は続けられるので、
 * 失敗を他の機能へ波及させない。効かなかったことは console.debug に残す。
 */
export const applyChatDisplay = (
  settings: ChatDisplaySettings,
  host: StyleHost = document,
): void => {
  const root = host.documentElement;
  if (root === null) {
    debug("差し込み先の要素が無い");
    return;
  }
  try {
    for (const [property, value] of Object.entries(chatDisplayVariables(settings))) {
      root.style.setProperty(property, value);
    }
    // getElementById は文書に繋がっている要素しか返さないので、外されていれば入れ直しになる。
    if (host.getElementById(CHAT_DISPLAY_STYLE_ID) !== null) return;
    const style = host.createElement("style");
    style.id = CHAT_DISPLAY_STYLE_ID;
    style.textContent = CHAT_DISPLAY_CSS;
    // document_start では <head> がまだ無いので、根の要素の末尾へ入れる。
    root.insertAdjacentElement("beforeend", style);
  } catch (error) {
    debug("適用に失敗した", error);
  }
};

/** 繋ぎ先。既定は実ブラウザのもので、テストはすべて差し替える。 */
export type ChatDisplayOptions = {
  readonly store?: SettingsStore;
  readonly host?: StyleHost;
  readonly navigate?: (apply: () => void) => void;
};

/**
 * 保存値を読んで当て、以後は変更と遷移のたびに当て直す。
 *
 * watch ページとライブチャットの iframe の両方でこの content script が走り、それぞれが
 * 自分の文書へ当てる。どちらの規則が効くかは CSS のセレクタが決めるので、面（watch か
 * live_chat か）をここで見分けない。
 *
 * 当てる順序は問わない。CSS は上書きで戻せるので、保存値が届く前に既定値が当たっても
 * 取り返しがつく（DOM から消す整理とはそこが違う）。
 */
export const startChatDisplay = ({
  store = localSettingsStore,
  host = document,
  navigate = onNavigated,
}: ChatDisplayOptions = {}): void => {
  const applyFromStore = (): Promise<void> =>
    applySection(store, chatDisplaySection, (settings) =>
      applyChatDisplay(settings, host),
    );

  // document_start では onNavigated が DOMContentLoaded まで初回を遅らせるので、そこを待たずに当てる。
  void applyFromStore();

  watchSection(store, chatDisplaySection, (settings) =>
    applyChatDisplay(settings, host),
  );

  // 遷移では文書が作り直されないので、当てた CSS が残っているとは限らない。当て直す。
  navigate(() => {
    void applyFromStore();
  });
};
