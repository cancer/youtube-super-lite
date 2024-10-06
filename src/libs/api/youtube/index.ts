import type { ApiClient } from "~/libs/api/youtube/client";
import type {
  PageInfo,
  Subscription,
  VideoGetRatingResponse,
} from "~/libs/api/youtube/types";

export type MyChannelsRequest = {
  GET: { part: string[]; maxResults: number };
};
export type MyChannelsResponse = {
  GET: PageInfo & {
    items: Subscription[];
  };
};
export const listMyChannels = (client: ApiClient) => {
  "use server";
  return ({
    part,
    maxResults,
  }: MyChannelsRequest["GET"]): Promise<MyChannelsResponse["GET"]> => {
    const params = {
      maxResults,
      part: part.join(","),
      mine: true,
    };
    return client.request({
      uri: "/subscriptions",
      method: "GET",
      params,
    });
  };
};

export type VideoRatingRequest = {
  GET: { id: string };
  POST: { id: string; rating: "like" };
};
export type VideoRatingResponse = {
  GET: { rating: string };
  POST: unknown;
};
export const getVideoRating = (client: ApiClient) => {
  "use server";
  return async ({
    id,
  }: VideoRatingRequest["GET"]): Promise<VideoRatingResponse["GET"]> => {
    "use server";
    const params = {
      id,
    };
    return client
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
};
export const postVideoRating = (client: ApiClient) => {
  "use server";
  return async (
    params: VideoRatingRequest["POST"],
  ): Promise<VideoRatingResponse["POST"]> => {
    "use server";

    return client.request<unknown>({
      uri: "/videos/rate",
      method: "POST",
      params,
    });
  };
};
