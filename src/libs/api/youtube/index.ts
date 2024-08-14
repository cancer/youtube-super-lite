import { type ApiClient, createApiClient } from "~/libs/api/youtube/client";
import type {
  Channel,
  ListResponse,
  PageInfo,
  PlaylistItem,
  Subscription,
  Video,
  VideoGetRatingResponse,
} from "~/libs/api/youtube/types";
import { createAuthTokensClient } from "~/libs/auth-tokens/client";
import { getCloudflareEnv } from "~/libs/cloudflare";
import { getSession } from "~/libs/session";

// XXX: ほんとはこんなところでやりたくないが、コンポーネント経由で渡そうとするとシリアライズの問題が出るのでできない
// TODO: middlewareでやる
let memo: ApiClient | null = null;
const client = () => {
  if (memo !== null) return memo;
  const authTokensClient = createAuthTokensClient(() =>
    getCloudflareEnv().then((env) => getSession(env.SESSION_SECRET!)),
  );
  memo = createApiClient({
    getTokens() {
      return authTokensClient.get();
    },
    async revokeTokens() {
      await authTokensClient.clear();
    },
  });
  return memo;
};

export type MyChannelsRequest = {
  GET: { part: string[]; maxResults: number };
};
export type MyChannelsResponse = {
  GET: PageInfo & {
    items: Subscription[];
  };
};
export const listMyChannels = ({
  part,
  maxResults,
}: MyChannelsRequest["GET"]): Promise<MyChannelsResponse["GET"]> => {
  "use server";
  const params = {
    maxResults,
    part: part.join(","),
    mine: true,
  };
  return client().request({
    uri: "/subscriptions",
    method: "GET",
    params,
  });
};

export type LatestVideoListRequestGet = {
  maxResults: number;
  publishedAfter: Date;
};
export type LatestVideoListResponseGet = ListResponse<
  Pick<Video, "id" | "snippet" | "contentDetails" | "liveStreamingDetails"> & {
    isShorts: boolean;
  }
>;
export const listLatestVideos = async ({
  maxResults,
  publishedAfter,
}: LatestVideoListRequestGet): Promise<LatestVideoListResponseGet> => {
  "use server";
  // まずはすべての登録チャンネルを取ってきて、
  const channels: ListResponse<Pick<Subscription, "snippet">> =
    await client().request({
      uri: "/subscriptions",
      method: "GET",
      params: {
        maxResults,
        part: "snippet",
        mine: true,
      },
    });

  // 最新の動画を集めてくる（ライブ含む）
  const channelIds = channels.items.map(
    ({
      snippet: {
        resourceId: { channelId },
      },
    }) => channelId,
  );
  const latestUploadPlaylists = await client()
    .request<ListResponse<Pick<Channel, "snippet" | "contentDetails">>>({
      uri: "/channels",
      method: "GET",
      params: {
        part: "snippet, contentDetails",
        id: channelIds.join(","),
      },
    })
    .then(({ items }) =>
      items.map(
        ({ contentDetails }) => contentDetails.relatedPlaylists.uploads,
      ),
    );
  const latestUploads = await Promise.all(
    latestUploadPlaylists.map((playlistId) =>
      client()
        .request<ListResponse<Pick<PlaylistItem, "contentDetails">>>({
          uri: "/playlistItems",
          method: "GET",
          params: {
            part: "contentDetails",
            playlistId,
          },
        })
        .then(({ items }) => items),
    ),
  );

  // アップロード日順にソートしたvideoIdを集めて
  const videoIds = latestUploads
    .flat()
    .sort((a, b) => {
      if (a.contentDetails.videoPublishedAt < b.contentDetails.videoPublishedAt)
        return 1;
      if (a.contentDetails.videoPublishedAt > b.contentDetails.videoPublishedAt)
        return -1;
      return 0;
    })
    .reduce(
      (acc: string[], { contentDetails: { videoId, videoPublishedAt } }) => {
        if (new Date(videoPublishedAt) < publishedAfter) return acc;
        acc.push(videoId);
        return acc;
      },
      [],
    )
    .slice(0, maxResults);

  // video詳細をリクエスト
  return await client().request<LatestVideoListResponseGet>({
    uri: "/videos",
    method: "GET",
    params: {
      part: "id,snippet, contentDetails, liveStreamingDetails",
      id: videoIds.join(","),
      maxResults,
    },
  });
};

export type VideoRatingRequest = {
  GET: { id: string };
  POST: { id: string; rating: "like" };
};
export type VideoRatingResponse = {
  GET: { rating: string };
  POST: unknown;
};
export const getVideoRating = async ({
  id,
}: VideoRatingRequest["GET"]): Promise<VideoRatingResponse["GET"]> => {
  "use server";
  const params = {
    id,
  };
  return client()
    .request<VideoGetRatingResponse>({
      uri: "/videos/getRating",
      method: "GET",
      params,
    })
    .then((res) => {
      if (res.items.length === 0) return { rating: "" };
      return { rating: res.items[0].rating };
    });
};
export const postVideoRating = async (
  params: VideoRatingRequest["POST"],
): Promise<VideoRatingResponse["POST"]> => {
  "use server";

  return client().request<unknown>({
    uri: "/videos/rate",
    method: "POST",
    params,
  });
};
