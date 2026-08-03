import {
  CHAT_PANEL_WIDTH_RATIO,
  chatDisplaySection,
  clampToRange,
  localSettingsStore,
  patchSection,
  type SettingsStore,
} from "../shared/settings";
import {
  applyPanelWidth,
  notifyLayoutChanged,
  type LayoutView,
  type StyleHost,
} from "./chat-display";

/**
 * チャットの幅を、watch ページの上で掴んで変える（要件 R5 の操作面）。
 *
 * 幅はサイドパネルのスライダーではなくページ内のハンドルで変える（ユーザー決定 2026-08-02）。
 * 幅は見ている画面を見ながら決めるものなので、操作する場所も見ている場所に置く。
 *
 * 幅の当て方（どの要素に効かせるか）は chat-display が持ち、ここは「掴んだぶんをどの値にするか」と
 * 「いつ保存するか」だけを決める。掴んでいる間の見た目はその場で当て、保存は離した時に 1 度行なう。
 * 動かすたびに保存すると、保存の往復のぶん幅が手から遅れて付いてくる。
 *
 * 1 カラム表示（窓が狭いとき）ではチャットが右の列の外へ出て、幅の指定そのものが効かなくなる。
 * ハンドルはチャットと同じ場所に付いて回るので出たままだが、動かしても幅は変わらない。これは
 * 幅の設定がもともと 2 カラム表示だけのものだから（chat-display の CHAT_LAYOUT_SELECTOR 参照）。
 */

/** ハンドルの id。同じものを二重に差し込まないための目印。 */
export const CHAT_RESIZE_HANDLE_ID = "youtube-super-lite-chat-resize";

/**
 * ハンドルを差し込む先。
 *
 * チャットの枠そのものを選ぶ。掴んだ時点の幅をこの要素から測り、ハンドルもこの要素の左端へ
 * 重ねるので、置き先は「画面に見えているチャットの箱」でなければならない。
 *
 * 出所: 2026-08-02〜03 に実ブラウザ 2 台（別レイアウト・シアター表示と既定表示の両方）で確認。
 * 包んでいる `#chat-container` は選べない。シアター表示ではチャットが固定配置で浮き、
 * `#chat-container` は幅 0 の目印だけになるため（実測 clientWidth 0）、そちらへ付けるとハンドルが
 * チャットの外へ出て、掴んだ幅も 0 になる。枠自体はどの表示でも幅を持つ（実測 255〜720px）。
 *
 * watch ページのチャットだけを指す（`ytd-watch-flexy` の中）。この content script は
 * ライブチャットの iframe にも入るが、そちらの文書には一致する要素が無い。
 */
export const CHAT_RESIZE_HOST_SELECTOR = "ytd-watch-flexy ytd-live-chat-frame";

/**
 * 差し込み先を引き直す間隔（ミリ秒）。
 *
 * 差し込み先は document_start では無く、遷移でも作り直される。掴んだ参照は持ち越せないので
 * 引き直す。周期で引き直すだけなら、出現待ちと遷移後の張り直しという状態を持たずに同じ効果が
 * 得られる（chat-trim と同じ理由）。
 */
export const CHAT_RESIZE_RETRY_INTERVAL_MS = 1000;

/**
 * ハンドルの見た目と当たり判定。
 *
 * チャットの左端の「内側」へ重ねる。外（チャットと動画の隙間）へ出すと、列の `overflow: auto` に
 * 切り取られて掴めない（2026-08-02 実測）。内側でもチャットの発言は左に余白を持つので、文字の上に
 * は乗らない。
 *
 * `z-index` はチャットの中身（iframe）より前に出す。前に出さないと、重なっているぶんの当たり判定を
 * 中身に取られて掴めない。`pointer-events` を自分で宣言するのは、親から切られている面（列全体を
 * `pointer-events: none` にしている版がある）でも掴めるようにするため。
 */
const HANDLE_STYLE: Readonly<Record<string, string>> = {
  position: "absolute",
  left: "0",
  top: "0",
  bottom: "0",
  width: "6px",
  cursor: "col-resize",
  "z-index": "900",
  "pointer-events": "auto",
  // 明るい配色でも暗い配色でも見える灰色。掴める場所が見えないと、ハンドルは無いのと同じ。
  background: "rgba(128, 128, 128, 0.35)",
  // ドラッグをページのスクロールへ流さない（触操作で掴めるようにする）。
  "touch-action": "none",
};

/** ハンドルの説明。掴めることを見た目以外でも伝える。 */
const HANDLE_TITLE = "左右にドラッグしてチャットの幅を変える";

/** ドラッグ 1 つ分の入力。実体は PointerEvent。 */
export type DragPointer = {
  readonly clientX: number;
  readonly pointerId: number;
  preventDefault(): void;
};

/** ドラッグを受けるつまみ。実体は `<div>`。 */
export type Handle = {
  id: string;
  title: string;
  readonly style: { setProperty(property: string, value: string): void };
  setAttribute(name: string, value: string): void;
  addEventListener(
    type: "pointerdown" | "pointermove" | "pointerup" | "pointercancel",
    listener: (event: DragPointer) => void,
  ): void;
  /** 掴んでいる間、ポインタが他の要素（チャットの iframe）へ出てもイベントを受け取り続ける。 */
  setPointerCapture(pointerId: number): void;
};

/**
 * 差し込みの引数。
 *
 * 実 Element を当てはめられる最小の形にしてある。Handle をそのまま要求すると、実 DOM の
 * `insertAdjacentElement`（引数は `Element`）を この型へ当てはめられない。
 */
type InsertableElement = { id: string };

/**
 * ハンドルを差し込む先。実体は watch ページのライブチャットの枠。
 *
 * 幅の現在値（clientWidth）を訊くのは、掴んだ時点の幅を基準にするため。差し込み済みかを
 * `querySelector` で訊くのは、YouTube 側の作り直しでハンドルごと消えることがあるため。
 *
 * 相手の style は書き換えない。ハンドルは絶対配置なので位置の基準になる祖先が要るが、チャットの枠は
 * どの表示でも配置済み（実測: 既定表示で relative、シアター表示で fixed）で、指定を足す必要が無い。
 * それどころか `position` を上書きすると、固定配置で浮いているシアター表示のチャットが元の場所へ
 * 落ちてレイアウトが崩れる（実測）。
 */
export type HandleHost = {
  readonly clientWidth: number;
  querySelector(selectors: string): unknown;
  insertAdjacentElement(position: "beforeend", element: InsertableElement): unknown;
};

/**
 * 掴んだ点から動いたぶんを幅の比率にする。
 *
 * 列は右端が固定で左端が動くので、左へ動かすほど広くなる。掴んだ時点の幅と位置を基準にするため、
 * ハンドルの太さのぶん幅が飛ばず、掴んだ点がそのまま指に付いてくる。
 *
 * 比率は `100vw` に対する割合で、1vw は画面幅（スクロールバーを含む）の 1%。割る値も同じ幅を
 * 使わないと、掴んだ点と列の端が少しずつずれていく。
 *
 * 画面幅が 0 で来ると比率が数値にならないが、範囲へ収める側が既定値へ落とすので値は必ず範囲内。
 */
export const draggedRatio = (
  grip: { readonly clientX: number; readonly widthPx: number },
  clientX: number,
  viewportWidthPx: number,
): number =>
  clampToRange(
    CHAT_PANEL_WIDTH_RATIO,
    (grip.widthPx + grip.clientX - clientX) / viewportWidthPx,
  );

/** 繋ぎ先。既定は実ブラウザのもので、テストはすべて差し替える。 */
export type ChatResizeOptions = {
  /** このタブの保存先。 */
  readonly store?: SettingsStore;
  /** 次に開くタブが最初に使う値の保存先。 */
  readonly persistent?: SettingsStore;
  /** 差し込み先を探す。遷移で作り直されるので、掴むたびに引き直す。 */
  readonly findHost?: () => HandleHost | null;
  /** つまみを作る。 */
  readonly createHandle?: () => Handle;
  /** 幅を当てる先の文書。 */
  readonly styleHost?: StyleHost;
  /** 幅が変わったことを知らせる先。 */
  readonly view?: LayoutView;
  /** 画面幅（スクロールバーを含む）。 */
  readonly viewportWidth?: () => number;
  /** 差し込みを引き直す周期。実体は setInterval。 */
  readonly schedule?: (task: () => void, intervalMs: number) => void;
};

/**
 * ハンドルを置いて、ドラッグを幅の設定へ繋ぐ。
 *
 * 保存は「このタブ」と「次に開くタブの初期値」の両方へ書く。サイドパネルでの操作と同じ扱いで、
 * ここで決めた幅が次のタブでも初期値になる。
 */
export const startChatResize = ({
  store = localSettingsStore,
  persistent = localSettingsStore,
  findHost = () => document.querySelector<HTMLElement>(CHAT_RESIZE_HOST_SELECTOR),
  createHandle = () => document.createElement("div"),
  styleHost = document,
  view = window,
  viewportWidth = () => window.innerWidth,
  schedule = setInterval,
}: ChatResizeOptions = {}): void => {
  const handle = createHandle();
  handle.id = CHAT_RESIZE_HANDLE_ID;
  handle.title = HANDLE_TITLE;
  handle.setAttribute("role", "separator");
  handle.setAttribute("aria-orientation", "vertical");
  for (const [property, value] of Object.entries(HANDLE_STYLE)) {
    handle.style.setProperty(property, value);
  }

  /** 掴んでいる間だけ入る、基準の幅と位置。 */
  let grip: { readonly clientX: number; readonly widthPx: number } | undefined;
  /** 掴んでから当てた最後の比率。動かしていなければ入らない。 */
  let dragged: number | undefined;

  /**
   * 幅を保存する。
   *
   * 書き換えるのは幅のフィールドだけにする。区画ごと書き戻すと、同じ区画にある文字サイズ
   * （操作するのはサイドパネル）を巻き込む。保存がまだ無い保存先では読み出しが既定値を返すので、
   * 幅を動かしただけで文字サイズが既定値に戻ってしまう。
   */
  const save = async (panelWidthRatio: number): Promise<void> => {
    // 今見ているタブへ先に当てる。次に開くタブの初期値は、このタブの見た目には関係しないので後。
    if (store !== persistent) {
      await patchSection(store, chatDisplaySection, { panelWidthRatio });
    }
    await patchSection(persistent, chatDisplaySection, { panelWidthRatio });
  };

  handle.addEventListener("pointerdown", (event) => {
    const host = findHost();
    if (host === null) return;
    grip = { clientX: event.clientX, widthPx: host.clientWidth };
    handle.setPointerCapture(event.pointerId);
    // 掴んだところからページ側の文字選択が始まるのを止める。
    event.preventDefault();
  });

  handle.addEventListener("pointermove", (event) => {
    if (grip === undefined) return;
    dragged = draggedRatio(grip, event.clientX, viewportWidth());
    applyPanelWidth(dragged, styleHost);
  });

  /**
   * 掴んだ手を離す。
   *
   * 中断（pointercancel）でも同じ扱いにする。中断された時点の幅は既に当たっているので、
   * 保存しないと次に開いたときだけ戻る、という食い違いになる。
   *
   * プレーヤーに測り直させるのは離したときだけにする。動かしている間に毎回知らせると、
   * その都度 YouTube の再レイアウトが走って手の動きが重くなる。掴んでいる間に追随するのは
   * チャットの幅で、映像の大きさは手を離してから揃う。
   */
  const release = (): void => {
    if (grip === undefined) return;
    grip = undefined;
    if (dragged === undefined) return;
    void save(dragged);
    dragged = undefined;
    notifyLayoutChanged(view);
  };

  handle.addEventListener("pointerup", release);
  handle.addEventListener("pointercancel", release);

  const attach = (): void => {
    const host = findHost();
    if (host === null || host.querySelector(`#${CHAT_RESIZE_HANDLE_ID}`) !== null) {
      return;
    }
    host.insertAdjacentElement("beforeend", handle);
  };

  attach();
  schedule(attach, CHAT_RESIZE_RETRY_INTERVAL_MS);
};
