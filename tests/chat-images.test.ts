import { describe, expect, test } from "bun:test";

import {
  registerChatImages,
  showsAuthorPhoto,
  stripChatImages,
} from "../src/main/chat-images";
import { installIntercept, type JsonTransform } from "../src/main/intercept";
import type { TransformTarget } from "../src/shared/endpoints";

/**
 * ライブチャット応答の画像除去（R3 のデータ層）。
 *
 * 期待値の出所は 2026-07-27 / 07-28 に実配信とアーカイブから採取した応答で、
 * このファイルの入力はその外形をそのまま写してある。実測から分かっている落とし穴
 * （バッジは配列で複数入る / アバターは authorPhoto 以外にもある / リプレイは階層が 1 つ深い）を
 * それぞれ 1 つのテストで固定する。
 */

/** 応答から値を取り出す。存在しない経路は undefined になる。 */
const at = (root: unknown, ...path: readonly (string | number)[]): unknown =>
  path.reduce<unknown>(
    (node, key) =>
      typeof node === "object" && node !== null
        ? (node as Record<string, unknown>)[key]
        : undefined,
    root,
  );

/** 配信中（get_live_chat）の外形。actions[] が addChatItemAction を直接持つ。 */
const liveEnvelope = (...items: unknown[]): unknown => ({
  responseContext: {},
  continuationContents: {
    liveChatContinuation: {
      continuations: [{ invalidationContinuationData: { timeoutMs: 10000 } }],
      actions: items.map((item) => ({ addChatItemAction: { item, clientId: "x" } })),
    },
  },
});

const liveItem = (index: number): readonly (string | number)[] => [
  "continuationContents",
  "liveChatContinuation",
  "actions",
  index,
  "addChatItemAction",
  "item",
];

const LIVE_ITEM = liveItem(0);

/** アーカイブ（get_live_chat_replay）の外形。replayChatItemAction の階層が 1 つ多い。 */
const replayEnvelope = (...items: unknown[]): unknown => ({
  responseContext: {},
  continuationContents: {
    liveChatContinuation: {
      actions: items.map((item) => ({
        replayChatItemAction: {
          actions: [{ addChatItemAction: { item, clientId: "x" } }],
          videoOffsetTimeMsec: "76493",
        },
      })),
    },
  },
});

const REPLAY_ITEM = [
  "continuationContents",
  "liveChatContinuation",
  "actions",
  0,
  "replayChatItemAction",
  "actions",
  0,
  "addChatItemAction",
  "item",
] as const;

// 変換は木を書き換えるので、入力は毎回作り直す（テスト間で共有しない）。
const authorPhoto = (): unknown => ({
  thumbnails: [
    { url: "https://yt4.ggpht.com/ytc/AIdro_k=s32-c-k-c0x00ffffff-no-rj", width: 32, height: 32 },
    { url: "https://yt4.ggpht.com/ytc/AIdro_k=s64-c-k-c0x00ffffff-no-rj", width: 64, height: 64 },
  ],
});

const iconBadge = (iconType: string): unknown => ({
  liveChatAuthorBadgeRenderer: {
    icon: { iconType },
    tooltip: iconType,
    accessibility: { accessibilityData: { label: iconType } },
  },
});

/** メンバーバッジ。icon を持たず customThumbnail を使うので iconType 判定に掛からない。 */
const memberBadge = (): unknown => ({
  liveChatAuthorBadgeRenderer: {
    customThumbnail: {
      thumbnails: [{ url: "https://yt3.ggpht.com/DP-JlV=s16-c-k", width: 16, height: 16 }],
    },
    tooltip: "メンバー（1 年）",
  },
});

const textMessage = (...badges: unknown[]): unknown => ({
  liveChatTextMessageRenderer: {
    message: { runs: [{ text: "ドゥフドゥフドゥフ" }] },
    authorName: { simpleText: "@viewer" },
    authorPhoto: authorPhoto(),
    id: "ChwKGkNQT0g0dmlM",
    timestampUsec: "1785163412971358",
    ...(badges.length === 0 ? {} : { authorBadges: badges }),
  },
});

const MESSAGE = "liveChatTextMessageRenderer";

/**
 * アバターを表示するかの判定そのもの。
 *
 * 判定は 1 つの純関数に閉じてあるので、除去の副作用ごしではなくここで直接固定する。
 * 「バッジが何であれば表示するか」は要件がユーザーの決定として持っている仕様で、
 * 実装の都合で変わってよい部分ではない。
 */
describe("showsAuthorPhoto", () => {
  test("MODERATOR バッジがあれば表示する", () => {
    expect(showsAuthorPhoto([iconBadge("MODERATOR")])).toBe(true);
  });

  test("OWNER バッジがあれば表示する", () => {
    expect(showsAuthorPhoto([iconBadge("OWNER")])).toBe(true);
  });

  test("バッジが無ければ表示しない", () => {
    expect(showsAuthorPhoto(undefined)).toBe(false);
  });

  test("VERIFIED だけなら表示しない", () => {
    expect(showsAuthorPhoto([iconBadge("VERIFIED")])).toBe(false);
  });

  /**
   * メンバーバッジは icon を持たず customThumbnail で来る。iconType を見る判定から外れるのは
   * 実装の副作用ではなく満たすべき仕様なので、判定の側で明示的に固定する。
   */
  test("メンバーバッジ（customThumbnail）は表示の根拠にならない", () => {
    expect(showsAuthorPhoto([memberBadge()])).toBe(false);
  });

  test("メンバーバッジと MODERATOR が並んでいれば表示する", () => {
    expect(showsAuthorPhoto([memberBadge(), iconBadge("MODERATOR")])).toBe(true);
  });

  // 実測で [VERIFIED, MODERATOR] を確認している。先頭だけを見る判定はここで落ちる。
  test("2 番目以降の MODERATOR も拾う", () => {
    expect(showsAuthorPhoto([iconBadge("VERIFIED"), iconBadge("MODERATOR")])).toBe(true);
  });

  test("未知の iconType では表示しない", () => {
    expect(showsAuthorPhoto([iconBadge("SPONSOR")])).toBe(false);
  });

  test("配列でない値が来ても表示しない", () => {
    expect(showsAuthorPhoto({ unexpected: true })).toBe(false);
    expect(showsAuthorPhoto(null)).toBe(false);
  });

  test("バッジの中身が想定外の形でも表示しない", () => {
    expect(showsAuthorPhoto([{}, null, { liveChatAuthorBadgeRenderer: {} }])).toBe(false);
  });
});

describe("アバターの選択的除去", () => {
  test("MODERATOR の投稿は authorPhoto が残る", () => {
    const result = stripChatImages(liveEnvelope(textMessage(iconBadge("MODERATOR"))));

    expect(at(result, ...LIVE_ITEM, MESSAGE, "authorPhoto", "thumbnails")).toHaveLength(2);
  });

  test("OWNER の投稿は authorPhoto が残る", () => {
    const result = stripChatImages(liveEnvelope(textMessage(iconBadge("OWNER"))));

    expect(at(result, ...LIVE_ITEM, MESSAGE, "authorPhoto", "thumbnails")).toHaveLength(2);
  });

  test("バッジの無い一般視聴者の投稿は authorPhoto が落ちる", () => {
    const result = stripChatImages(liveEnvelope(textMessage()));

    expect(at(result, ...LIVE_ITEM, MESSAGE, "authorPhoto")).toBeUndefined();
  });

  // メンバーバッジは icon ではなく customThumbnail で来るので、iconType 判定で自然に外れる。
  test("メンバーバッジだけの投稿は authorPhoto が落ちる", () => {
    const result = stripChatImages(liveEnvelope(textMessage(memberBadge())));

    expect(at(result, ...LIVE_ITEM, MESSAGE, "authorPhoto")).toBeUndefined();
  });

  test("認証済みチャンネルの投稿は authorPhoto が落ちる", () => {
    const result = stripChatImages(liveEnvelope(textMessage(iconBadge("VERIFIED"))));

    expect(at(result, ...LIVE_ITEM, MESSAGE, "authorPhoto")).toBeUndefined();
  });

  // 実測で [VERIFIED, MODERATOR] を確認している。authorBadges[0] だけを見る実装はここで落ちる。
  test("authorBadges の 2 番目が MODERATOR でも authorPhoto が残る", () => {
    const result = stripChatImages(
      liveEnvelope(textMessage(iconBadge("VERIFIED"), iconBadge("MODERATOR"))),
    );

    expect(at(result, ...LIVE_ITEM, MESSAGE, "authorPhoto", "thumbnails")).toHaveLength(2);
  });

  // ピン留めは addChatItemAction の外に居る。判定を addChatItemAction の下に限ると漏れる。
  test("ピン留めバナーの OWNER 投稿は authorPhoto が残る", () => {
    const banner = {
      responseContext: {},
      continuationContents: {
        liveChatContinuation: {
          actions: [
            {
              addBannerToLiveChatCommand: {
                bannerRenderer: {
                  liveChatBannerRenderer: {
                    contents: textMessage(iconBadge("OWNER")),
                  },
                },
              },
            },
          ],
        },
      },
    };

    const result = stripChatImages(banner);

    expect(
      at(
        result,
        "continuationContents",
        "liveChatContinuation",
        "actions",
        0,
        "addBannerToLiveChatCommand",
        "bannerRenderer",
        "liveChatBannerRenderer",
        "contents",
        MESSAGE,
        "authorPhoto",
        "thumbnails",
      ),
    ).toHaveLength(2);
  });

  test("ピン留めバナーの一般視聴者の投稿は authorPhoto が落ちる", () => {
    const banner = {
      continuationContents: {
        liveChatContinuation: {
          actions: [
            {
              addBannerToLiveChatCommand: {
                bannerRenderer: { liveChatBannerRenderer: { contents: textMessage() } },
              },
            },
          ],
        },
      },
    };

    const result = stripChatImages(banner);

    expect(
      at(
        result,
        "continuationContents",
        "liveChatContinuation",
        "actions",
        0,
        "addBannerToLiveChatCommand",
        "bannerRenderer",
        "liveChatBannerRenderer",
        "contents",
        MESSAGE,
        "authorPhoto",
      ),
    ).toBeUndefined();
  });

  // giftMessageViewModel は *Renderer ではなく、鍵も authorPhoto ではない。
  test("ギフトの authorAvatar から画像 URL が落ちる", () => {
    const gift = {
      giftMessageViewModel: {
        text: { content: "かき氷 を送信しました" },
        authorName: { content: "@バイオインパクト " },
        authorAvatar: {
          avatarViewModel: {
            image: {
              sources: [{ url: "https://yt4.ggpht.com/a=s32", width: 32, height: 32 }],
              processor: { borderImageProcessor: { circular: true } },
            },
            avatarImageSize: "AVATAR_SIZE_XS",
          },
        },
      },
    };

    const result = stripChatImages(liveEnvelope(gift));

    expect(
      at(
        result,
        ...LIVE_ITEM,
        "giftMessageViewModel",
        "authorAvatar",
        "avatarViewModel",
        "image",
        "sources",
      ),
    ).toBeUndefined();
  });

  test("ティッカーの sponsorPhoto から画像 URL が落ちる", () => {
    const ticker = {
      responseContext: {},
      continuationContents: {
        liveChatContinuation: {
          actions: [
            {
              addLiveChatTickerItemAction: {
                item: {
                  liveChatTickerSponsorItemRenderer: {
                    detailText: { simpleText: " " },
                    sponsorPhoto: {
                      thumbnails: [{ url: "https://yt4.ggpht.com/ytc/b=s32", width: 32 }],
                    },
                    durationSec: 178,
                  },
                },
              },
            },
          ],
        },
      },
    };

    const result = stripChatImages(ticker);

    expect(
      at(
        result,
        "continuationContents",
        "liveChatContinuation",
        "actions",
        0,
        "addLiveChatTickerItemAction",
        "item",
        "liveChatTickerSponsorItemRenderer",
        "sponsorPhoto",
        "thumbnails",
      ),
    ).toBeUndefined();
  });

  test("スーパーチャットに付く creatorThumbnail から画像 URL が落ちる", () => {
    const paid = {
      liveChatPaidMessageRenderer: {
        purchaseAmountText: { simpleText: "￥500" },
        authorPhoto: authorPhoto(),
        creatorHeartButton: {
          creatorHeartViewModel: {
            creatorThumbnail: {
              sources: [{ url: "https://yt3.ggpht.com/c=s48-c-k-c0x00ffffff-no-rj" }],
            },
            heartedHoverText: "@… さんが ❤ をつけました",
          },
        },
      },
    };

    const result = stripChatImages(liveEnvelope(paid));

    expect(
      at(
        result,
        ...LIVE_ITEM,
        "liveChatPaidMessageRenderer",
        "creatorHeartButton",
        "creatorHeartViewModel",
        "creatorThumbnail",
        "sources",
      ),
    ).toBeUndefined();
  });

  /**
   * メンバーバッジの画像（`customThumbnail`、16/32px）は落とさない。
   *
   * ユーザーの決定（2026-08-01）で「残す」。要件が落とすと定めた 3 カテゴリ（絵文字・スタンプ・
   * スーパーチャット装飾）に入らず、メンバーの見分けに使われるため。落ちる・落ちないのどちらとも
   * 決まっていない状態にしないよう、残ることを明示的に固定する。
   */
  test("メンバーバッジの画像は残る", () => {
    const result = stripChatImages(liveEnvelope(textMessage(memberBadge())));

    expect(
      at(
        result,
        ...LIVE_ITEM,
        MESSAGE,
        "authorBadges",
        0,
        "liveChatAuthorBadgeRenderer",
        "customThumbnail",
        "thumbnails",
      ),
    ).toHaveLength(1);
  });

  test("アバターを落とす投稿でもメンバーバッジの画像は残る", () => {
    const result = stripChatImages(liveEnvelope(textMessage(memberBadge())));

    expect(at(result, ...LIVE_ITEM, MESSAGE, "authorPhoto")).toBeUndefined();
    expect(
      at(
        result,
        ...LIVE_ITEM,
        MESSAGE,
        "authorBadges",
        0,
        "liveChatAuthorBadgeRenderer",
        "customThumbnail",
      ),
    ).toBeDefined();
  });

  test("参加者リストの一般参加者は authorPhoto が落ちる", () => {
    const payload = {
      contents: {
        liveChatRenderer: {
          participantsList: {
            liveChatParticipantsListRenderer: {
              participants: [
                { liveChatParticipantRenderer: { authorPhoto: authorPhoto() } },
                {
                  liveChatParticipantRenderer: {
                    authorPhoto: authorPhoto(),
                    authorBadges: [iconBadge("OWNER")],
                  },
                },
              ],
            },
          },
        },
      },
    };
    const participants = [
      "contents",
      "liveChatRenderer",
      "participantsList",
      "liveChatParticipantsListRenderer",
      "participants",
    ] as const;

    const result = stripChatImages(payload);

    expect(
      at(result, ...participants, 0, "liveChatParticipantRenderer", "authorPhoto"),
    ).toBeUndefined();
    expect(
      at(result, ...participants, 1, "liveChatParticipantRenderer", "authorPhoto", "thumbnails"),
    ).toHaveLength(2);
  });
});

describe("装飾画像の除去", () => {
  const emojiRun = (): unknown => ({
    emoji: {
      emojiId: "😂",
      shortcuts: [":face_with_tears_of_joy:", ":joy:"],
      searchTerms: ["face", "with", "tears", "of", "joy"],
      image: {
        thumbnails: [{ url: "https://fonts.gstatic.com/s/e/notoemoji/15.1/1f602/72.png" }],
        accessibility: { accessibilityData: { label: "😂" } },
      },
    },
  });

  const emojiMessage = (): unknown => ({
    liveChatTextMessageRenderer: {
      message: { runs: [{ text: "ドゥフ音よし!" }, emojiRun()] },
      authorName: { simpleText: "@viewer" },
      authorPhoto: authorPhoto(),
    },
  });

  const EMOJI = [...LIVE_ITEM, MESSAGE, "message", "runs", 1, "emoji"] as const;

  test("絵文字の画像 URL が落ちる", () => {
    const result = stripChatImages(liveEnvelope(emojiMessage()));

    expect(at(result, ...EMOJI, "image", "thumbnails")).toBeUndefined();
  });

  test("絵文字の shortcuts と代替テキストは残る", () => {
    const result = stripChatImages(liveEnvelope(emojiMessage()));

    expect(at(result, ...EMOJI, "shortcuts")).toEqual([
      ":face_with_tears_of_joy:",
      ":joy:",
    ]);
    expect(
      at(result, ...EMOJI, "image", "accessibility", "accessibilityData", "label"),
    ).toBe("😂");
  });

  test("発言本文のテキストは消さない", () => {
    const result = stripChatImages(liveEnvelope(emojiMessage()));

    expect(at(result, ...LIVE_ITEM, MESSAGE, "message", "runs", 0, "text")).toBe(
      "ドゥフ音よし!",
    );
  });

  const paidSticker = (): unknown => ({
    liveChatPaidStickerRenderer: {
      purchaseAmountText: { simpleText: "￥90" },
      authorPhoto: authorPhoto(),
      sticker: {
        thumbnails: [
          { url: "//lh3.googleusercontent.com/yAtGAw9ew", width: 40, height: 40 },
          { url: "//lh3.googleusercontent.com/yAtGAw9ew", width: 80, height: 80 },
        ],
        accessibility: { accessibilityData: { label: "スタンプの名前" } },
      },
      stickerDisplayWidth: 40,
      backgroundColor: 4280191205,
    },
  });

  const STICKER = [...LIVE_ITEM, "liveChatPaidStickerRenderer"] as const;

  test("スタンプの画像 URL が落ちる", () => {
    const result = stripChatImages(liveEnvelope(paidSticker()));

    expect(at(result, ...STICKER, "sticker", "thumbnails")).toBeUndefined();
  });

  test("スタンプの代替テキストは残る", () => {
    const result = stripChatImages(liveEnvelope(paidSticker()));

    expect(
      at(result, ...STICKER, "sticker", "accessibility", "accessibilityData", "label"),
    ).toBe("スタンプの名前");
  });

  // 装飾のうち画像でないもの（金額・色）は表示の手掛かりなので触らない。
  test("スーパーチャットの金額と色は残る", () => {
    const result = stripChatImages(liveEnvelope(paidSticker()));

    expect(at(result, ...STICKER, "purchaseAmountText", "simpleText")).toBe("￥90");
    expect(at(result, ...STICKER, "backgroundColor")).toBe(4280191205);
  });

  const gift = (): unknown => ({
    giftMessageViewModel: {
      text: { content: "かき氷 を送信しました" },
      giftImage: {
        sources: [
          { url: "//www.gstatic.com/youtube/img/pdg/gift/assets/shaved_ice.png=w480-h480", width: 480 },
          { url: "//www.gstatic.com/youtube/img/pdg/gift/assets/shaved_ice.png=w640-h640", width: 640 },
        ],
      },
      giftImageA11yLabel: "@… さんから かき氷 のギフトが送られました",
    },
  });

  const GIFT = [...LIVE_ITEM, "giftMessageViewModel"] as const;

  test("ギフト画像の URL が落ちる", () => {
    const result = stripChatImages(liveEnvelope(gift()));

    expect(at(result, ...GIFT, "giftImage", "sources")).toBeUndefined();
  });

  test("ギフトの説明テキストは残る", () => {
    const result = stripChatImages(liveEnvelope(gift()));

    expect(at(result, ...GIFT, "giftImageA11yLabel")).toBe(
      "@… さんから かき氷 のギフトが送られました",
    );
    expect(at(result, ...GIFT, "text", "content")).toBe("かき氷 を送信しました");
  });

  // ティッカーは有料項目の renderer を丸ごと入れ子で持ち直す。入れ子側も同じ規則が要る。
  test("ティッカーに入れ子のスタンプ renderer も処理する", () => {
    const ticker = {
      continuationContents: {
        liveChatContinuation: {
          actions: [
            {
              addLiveChatTickerItemAction: {
                item: {
                  liveChatTickerSponsorItemRenderer: {
                    showItemEndpoint: {
                      showLiveChatItemEndpoint: { renderer: paidSticker() },
                    },
                  },
                },
              },
            },
          ],
        },
      },
    };
    const nested = [
      "continuationContents",
      "liveChatContinuation",
      "actions",
      0,
      "addLiveChatTickerItemAction",
      "item",
      "liveChatTickerSponsorItemRenderer",
      "showItemEndpoint",
      "showLiveChatItemEndpoint",
      "renderer",
      "liveChatPaidStickerRenderer",
    ] as const;

    const result = stripChatImages(ticker);

    expect(at(result, ...nested, "sticker", "thumbnails")).toBeUndefined();
    expect(at(result, ...nested, "authorPhoto")).toBeUndefined();
  });
});

/**
 * バッジによる例外がどこまで効くか。
 *
 * 例外が掛かるのは `authorPhoto` だけで、`authorAvatar`・`sponsorPhoto`・`creatorThumbnail` は
 * 投稿者がモデレーターやオーナーでも落ちる。要件は「表示対象外の投稿者の authorPhoto を落とす」と
 * 鍵を名指しで定めており、ギフト・ティッカー・❤ の画像は残す対象ではなく、落とす側の
 * 「スーパーチャット装飾」に属するため。実装の取りこぼしと読み違えられないよう、経路ごとに固定する。
 */
describe("バッジの例外が掛からない画像", () => {
  test("モデレーターのスーパーチャットでも creatorThumbnail は落ちる", () => {
    const paid = {
      liveChatPaidMessageRenderer: {
        purchaseAmountText: { simpleText: "￥500" },
        authorPhoto: authorPhoto(),
        authorBadges: [iconBadge("MODERATOR")],
        creatorHeartButton: {
          creatorHeartViewModel: {
            creatorThumbnail: {
              sources: [{ url: "https://yt3.ggpht.com/c=s48-c-k-c0x00ffffff-no-rj" }],
            },
          },
        },
      },
    };
    const renderer = [...LIVE_ITEM, "liveChatPaidMessageRenderer"] as const;

    const result = stripChatImages(liveEnvelope(paid));

    expect(at(result, ...renderer, "authorPhoto", "thumbnails")).toHaveLength(2);
    expect(
      at(result, ...renderer, "creatorHeartButton", "creatorHeartViewModel", "creatorThumbnail", "sources"),
    ).toBeUndefined();
  });

  test("モデレーターのティッカー項目でも sponsorPhoto は落ちる", () => {
    const ticker = {
      liveChatTickerSponsorItemRenderer: {
        sponsorPhoto: {
          thumbnails: [{ url: "https://yt4.ggpht.com/ytc/b=s32", width: 32 }],
        },
        // バッジを同じ階層に置いても sponsorPhoto には効かない、というのがここで固定したい決定。
        authorBadges: [iconBadge("MODERATOR")],
        showItemEndpoint: {
          showLiveChatItemEndpoint: {
            renderer: textMessage(iconBadge("MODERATOR")),
          },
        },
      },
    };
    const item = [...LIVE_ITEM, "liveChatTickerSponsorItemRenderer"] as const;

    const result = stripChatImages(liveEnvelope(ticker));

    expect(at(result, ...item, "sponsorPhoto", "thumbnails")).toBeUndefined();
    // 入れ子の発言側は authorPhoto なので、同じ項目の中でも例外が効く。
    expect(
      at(
        result,
        ...item,
        "showItemEndpoint",
        "showLiveChatItemEndpoint",
        "renderer",
        MESSAGE,
        "authorPhoto",
        "thumbnails",
      ),
    ).toHaveLength(2);
  });

  // ギフトの authorAvatar は ViewModel 側の鍵で、バッジの有無を見ずに落とす。
  test("モデレーターのバッジが並んでいてもギフトの authorAvatar は落ちる", () => {
    const gift = {
      giftMessageViewModel: {
        text: { content: "かき氷 を送信しました" },
        authorBadges: [iconBadge("MODERATOR")],
        authorAvatar: {
          avatarViewModel: {
            image: { sources: [{ url: "https://yt4.ggpht.com/a=s32", width: 32 }] },
          },
        },
      },
    };

    const result = stripChatImages(liveEnvelope(gift));

    expect(
      at(result, ...LIVE_ITEM, "giftMessageViewModel", "authorAvatar", "avatarViewModel", "image", "sources"),
    ).toBeUndefined();
  });
});

/**
 * 1 つの actions[] に種類の違う項目が同居する場合。
 *
 * 実際のバッチは 1 項目では来ない。経路を列挙せず木を再帰で走る設計が意味を持つのはここで、
 * 兄弟のあいだで判定が持ち越されない（モデレーターの隣の一般発言が落ちる、その逆も）ことを固定する。
 */
describe("混在バッチ", () => {
  const giftItem = (): unknown => ({
    giftMessageViewModel: {
      text: { content: "かき氷 を送信しました" },
      giftImage: {
        sources: [{ url: "//www.gstatic.com/youtube/img/pdg/gift/assets/shaved_ice.png=w480-h480" }],
      },
      giftImageA11yLabel: "@… さんから かき氷 のギフトが送られました",
    },
  });

  test("モデレーターの次の一般発言は authorPhoto が落ちる", () => {
    const result = stripChatImages(
      liveEnvelope(textMessage(iconBadge("MODERATOR")), textMessage(), giftItem()),
    );

    expect(at(result, ...liveItem(0), MESSAGE, "authorPhoto", "thumbnails")).toHaveLength(2);
    expect(at(result, ...liveItem(1), MESSAGE, "authorPhoto")).toBeUndefined();
  });

  test("一般発言の次のモデレーター発言は authorPhoto が残る", () => {
    const result = stripChatImages(
      liveEnvelope(textMessage(), textMessage(iconBadge("MODERATOR")), textMessage()),
    );

    expect(at(result, ...liveItem(0), MESSAGE, "authorPhoto")).toBeUndefined();
    expect(at(result, ...liveItem(1), MESSAGE, "authorPhoto", "thumbnails")).toHaveLength(2);
    expect(at(result, ...liveItem(2), MESSAGE, "authorPhoto")).toBeUndefined();
  });

  test("同じバッチのギフトは、隣にモデレーターが居ても画像が落ちる", () => {
    const result = stripChatImages(
      liveEnvelope(textMessage(iconBadge("MODERATOR")), textMessage(), giftItem()),
    );

    expect(at(result, ...liveItem(2), "giftMessageViewModel", "giftImage", "sources")).toBeUndefined();
    expect(at(result, ...liveItem(2), "giftMessageViewModel", "giftImageA11yLabel")).toBe(
      "@… さんから かき氷 のギフトが送られました",
    );
  });

  test("メンバーバッジの画像は混在バッチでも残る", () => {
    const result = stripChatImages(
      liveEnvelope(textMessage(iconBadge("MODERATOR")), textMessage(memberBadge())),
    );

    expect(
      at(
        result,
        ...liveItem(1),
        MESSAGE,
        "authorBadges",
        0,
        "liveChatAuthorBadgeRenderer",
        "customThumbnail",
        "thumbnails",
      ),
    ).toHaveLength(1);
  });
});

describe("アーカイブ（replay）の外形", () => {
  test("replay の余分な階層の下でも一般視聴者の authorPhoto が落ちる", () => {
    const result = stripChatImages(replayEnvelope(textMessage()));

    expect(at(result, ...REPLAY_ITEM, MESSAGE, "authorPhoto")).toBeUndefined();
  });

  test("replay の余分な階層の下でも OWNER の authorPhoto が残る", () => {
    const result = stripChatImages(replayEnvelope(textMessage(iconBadge("OWNER"))));

    expect(at(result, ...REPLAY_ITEM, MESSAGE, "authorPhoto", "thumbnails")).toHaveLength(2);
  });

  test("replay の videoOffsetTimeMsec は残る", () => {
    const result = stripChatImages(replayEnvelope(textMessage()));

    expect(
      at(
        result,
        "continuationContents",
        "liveChatContinuation",
        "actions",
        0,
        "replayChatItemAction",
        "videoOffsetTimeMsec",
      ),
    ).toBe("76493");
  });
});

/**
 * 同じ木へ繰り返し当たっても結果が変わらないこと。
 *
 * 傍受層の XHR（responseType: "json"）の経路では、ページが `.response` を読むたびに同じ木へ
 * この変換が当たる。傍受層はそこに記憶を持たず、この冪等性に寄りかかっている。鍵を消す以外の
 * 操作（値の書き換え・追記）を足すとその前提が崩れるので、崩れたら落ちるようにしておく。
 */
describe("変換の冪等性", () => {
  const everything = (): unknown =>
    liveEnvelope(
      textMessage(iconBadge("MODERATOR")),
      textMessage(memberBadge()),
      {
        liveChatPaidStickerRenderer: {
          purchaseAmountText: { simpleText: "￥90" },
          authorPhoto: authorPhoto(),
          sticker: { thumbnails: [{ url: "//lh3.googleusercontent.com/y", width: 40 }] },
        },
      },
      {
        giftMessageViewModel: {
          giftImage: { sources: [{ url: "//www.gstatic.com/g.png=w480-h480" }] },
          giftImageA11yLabel: "ギフト",
        },
      },
    );

  test("2 回当てた結果が 1 回当てた結果と一致する", () => {
    const payload = everything();

    const once = structuredClone(stripChatImages(payload));
    const twice = stripChatImages(payload);

    expect(twice).toEqual(once);
  });

  test("2 回目で残すべきものが消えない", () => {
    const payload = everything();

    stripChatImages(payload);
    const result = stripChatImages(payload);

    expect(at(result, ...liveItem(0), MESSAGE, "authorPhoto", "thumbnails")).toHaveLength(2);
    expect(at(result, ...liveItem(3), "giftMessageViewModel", "giftImageA11yLabel")).toBe(
      "ギフト",
    );
  });
});

describe("想定外の入力への耐性", () => {
  test("JSON がオブジェクトでなくても throw しない", () => {
    expect(stripChatImages(null)).toBeNull();
    expect(stripChatImages("error")).toBe("error");
  });

  test("authorBadges が配列でなくても authorPhoto を落として throw しない", () => {
    const broken = liveEnvelope({
      liveChatTextMessageRenderer: {
        authorPhoto: authorPhoto(),
        authorBadges: { unexpected: true },
      },
    });

    expect(at(stripChatImages(broken), ...LIVE_ITEM, MESSAGE, "authorPhoto")).toBeUndefined();
  });
});

describe("傍受層への登録", () => {
  const LIVE_CHAT_URL =
    "https://www.youtube.com/youtubei/v1/live_chat/get_live_chat?prettyPrint=false";

  /** 登録内容だけを記録するレジストリ。傍受層を組み込まずに繋ぎ込みを見る。 */
  const recordingRegistry = (): {
    register: (target: TransformTarget, transform: JsonTransform) => void;
    readonly entries: ReadonlyArray<[TransformTarget, JsonTransform]>;
  } => {
    const entries: [TransformTarget, JsonTransform][] = [];
    return {
      register: (target, transform) => {
        entries.push([target, transform]);
      },
      get entries() {
        return entries;
      },
    };
  };

  test("live_chat を対象として変換を登録する", () => {
    const registry = recordingRegistry();

    registerChatImages(registry);

    expect(registry.entries).toEqual([["live_chat", stripChatImages]]);
  });

  test("live_chat の応答が傍受層を通って変換される", async () => {
    const payload = liveEnvelope(textMessage());
    const globals = {
      fetch: (_input: RequestInfo | URL): Promise<Response> =>
        Promise.resolve(
          new Response(JSON.stringify(payload), {
            headers: { "content-type": "application/json" },
          }),
        ),
      XMLHttpRequest: class {
        readyState = 0;
        responseType: XMLHttpRequestResponseType = "";
        open(): void {}
        get responseText(): string {
          return "";
        }
        get response(): unknown {
          return "";
        }
      },
    };
    registerChatImages(installIntercept(globals));

    const received = await (await globals.fetch(LIVE_CHAT_URL)).json();

    expect(at(received, ...LIVE_ITEM, MESSAGE, "authorPhoto")).toBeUndefined();
    expect(at(received, ...LIVE_ITEM, MESSAGE, "message", "runs", 0, "text")).toBe(
      "ドゥフドゥフドゥフ",
    );
  });
});
