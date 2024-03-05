import { cache, createAsync, useParams } from "@solidjs/router";
import { For, Show } from "solid-js";
import {
  type ChannelRequest,
  getChannel,
  listVideos,
  useYouTubeApiClient,
  type VideosRequest,
  type YouTubeApiClient,
} from "~/libs/api/youtube";

const fetchChannel = cache(
  async (client: YouTubeApiClient, params: ChannelRequest["GET"]) => {
    "use server";
    return getChannel(client)(params);
  },
  "channel",
);
const fetchVideos = cache(
  async (client: YouTubeApiClient, params: VideosRequest["GET"]) => {
    "use server";
    return listVideos(client)(params);
  },
  "videos",
);

const Channel = () => {
  const params = useParams();
  const apiClient = useYouTubeApiClient();
  const channel = createAsync(() =>
    fetchChannel(apiClient, {
      id: params.id,
      part: ["snippet"],
    }),
  );
  const videos = createAsync(() =>
    fetchVideos(apiClient, {
      channelId: params.id,
      maxResults: 50,
      order: "date",
      part: ["snippet"],
    }),
  );
  return (
    <Show when={videos()}>
      {(data) => (
        <For each={data().items}>
          {(video) => (
            <li>
              <a href={`https://www.youtube.com/watch?v=${video.id.videoId}`}>
                <img
                  src={video.snippet.thumbnails.default.url}
                  alt={video.snippet.title}
                />
              </a>
            </li>
          )}
        </For>
      )}
    </Show>
  );
};
export default Channel;
