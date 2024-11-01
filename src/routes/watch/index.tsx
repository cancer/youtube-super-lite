import {
  action,
  cache,
  createAsync,
  type RouteDefinition,
  useAction,
  useLocation,
  useNavigate,
  useSearchParams,
} from "@solidjs/router";
import { clientOnly } from "@solidjs/start";
import { createEffect, createMemo, createSignal, For, Show } from "solid-js";
import { getRequestEvent } from "solid-js/web";
import { getVideoRating, postVideoRating } from "~/libs/api/youtube";
import { isTokenExpired } from "~/libs/api/youtube/errors";
import { failed, pending, type QueryResult, succeed } from "~/libs/query";
import { parseYouTubeUrl } from "~/libs/url";
import { LikeButton } from "~/routes/watch/like-button";
import { Header } from "~/uis/header";
import { getLoginStatus, LoginButton, LogoutButton } from "~/uis/login-button";
import { WatchVideoFromYouTube } from "~/uis/watch-video-from-you-tube";

import "./styles.css";

const Player = clientOnly(() =>
  import("./player").then(({ Player }) => ({ default: Player })),
);

const fetchRatings = cache(
  async (params: {
    ids: string[];
  }): Promise<QueryResult<{ ratings: Map<string, boolean> }>> => {
    "use server";
    const event = getRequestEvent()!;
    let ratings: Map<string, boolean>;
    try {
      ratings = new Map(
        await Promise.all(
          params.ids.map((id) =>
            getVideoRating(event.locals.youtubeApi)({ id }).then(
              ({ rating }) => [id, rating === "like"] as const,
            ),
          ),
        ),
      );
    } catch (e) {
      return failed(e);
    }

    return succeed({ ratings });
  },
  "ratings",
);

const likeAction = action(async (id: string) => {
  "use server";
  const { youtubeApi } = getRequestEvent()!.locals;
  await postVideoRating(youtubeApi)({ id, rating: "like" });
  return null;
});

type Params = { videoIds: string };

export const routes = {
  load: async () => {
    const [{ videoIds }] = useSearchParams<Params>();
    if (!videoIds)
      return { done: true, success: true, data: { ratings: null } };
    return fetchRatings({ ids: videoIds.split(",") });
  },
} satisfies RouteDefinition;

const Watch = () => {
  const navigate = useNavigate();
  const location = useLocation();
  const [searchParams] = useSearchParams<Params>();

  const [videoIds, setVideoIds] = createSignal<string[]>(
    searchParams.videoIds?.split(",") ?? [],
  );
  const [liked, setLiked] = createSignal(new Map());

  const isLoggedIn = createAsync(() => getLoginStatus());
  const ratingsQuery = createAsync(
    async () => fetchRatings({ ids: videoIds() }),
    {
      deferStream: true,
      initialValue: pending(),
    },
  );

  const divisions = createMemo(() => Math.ceil(Math.sqrt(videoIds().length)));

  const likeMap = createMemo(() => {
    const query = ratingsQuery();
    if (!query.done) return new Map();
    if (!query.succeed) return new Map();
    return new Map(
      videoIds().map((id) => [
        id,
        liked().get(id) ?? query.data.ratings.get(id) ?? false,
      ]),
    );
  });

  const like = useAction(likeAction);

  createEffect(() => {
    const query = ratingsQuery();
    if (!query.done) return;
    if (query.succeed) return;
    if (!isTokenExpired(query.error)) return;

    window.location.assign(`/login?redirect_to=${window.location.href}`);
  });

  createEffect(() => {
    if (
      JSON.stringify(window.history.state.videoIds) ===
      JSON.stringify(videoIds())
    )
      return;
    if (videoIds().length === 0 && location.search === "") return navigate(".");
    // 履歴には残したいがre-renderはしたくない
    window.history.pushState(
      { videoIds: videoIds() },
      "",
      `?videoIds=${videoIds().join(",")}`,
    );
  });

  createEffect(() => {
    window.addEventListener(
      "popstate",
      () => {
        setVideoIds(window.history.state.videoIds ?? []);
      },
      true,
    );
  });

  return (
    <div class="w-screen h-screen grid grid-rows-[max-content_1fr] justify-center">
      <div class="w-screen h-max col-span-full">
        <Header
          LeftSide={
            <Show when={videoIds().length > 0}>
              <WatchVideoFromYouTube
                onSubmit={(ev) => {
                  ev.preventDefault();

                  if (ev.currentTarget.url.value === "") return;

                  // TODO: https://youtu.be/2wczkeeoYQc にも対応できるように
                  const parsed = parseYouTubeUrl(ev.currentTarget.url.value);
                  if (parsed.type !== "video") return;

                  if (
                    (ev.submitter as HTMLButtonElement).name === "openNewPage"
                  ) {
                    setVideoIds([parsed.id]);
                    ev.currentTarget.url.value = "";
                    return;
                  }

                  if (videoIds().length === 16)
                    return console.warn("Maximum number of videos reached.");

                  setVideoIds((prev) => [...prev, parsed.id]);
                  ev.currentTarget.url.value = "";
                }}
                Action={
                  <>
                    <button type="submit" name="openCurrentPage">
                      👇 Add
                    </button>
                    <button type="submit" name="openNewPage">
                      👉 Go
                    </button>
                  </>
                }
              ></WatchVideoFromYouTube>
            </Show>
          }
          RightSide={
            <Show when={isLoggedIn()} fallback={<LoginButton />}>
              <LogoutButton />
            </Show>
          }
        />
      </div>
      <div class="videoLayout">
        <div>
          <Show
            when={videoIds().length > 0 && videoIds()}
            fallback={
              <div class="noVideoLayout">
                <WatchVideoFromYouTube
                  onSubmit={(ev) => {
                    ev.preventDefault();

                    if (ev.currentTarget.url.value === "") return;

                    if (videoIds().length === 16)
                      return console.warn("Maximum number of videos reached.");

                    const parsed = parseYouTubeUrl(ev.currentTarget.url.value);
                    if (parsed.type !== "video") return;

                    setVideoIds((prev) => [...prev, parsed.id]);
                    ev.currentTarget.url.value = "";
                  }}
                  Action={<button type="submit">Watch</button>}
                ></WatchVideoFromYouTube>
              </div>
            }
            keyed
          >
            {(data) => (
              <div class="videoItemLayout" data-divisions={divisions()}>
                <For each={data}>
                  {(videoId) => (
                    <>
                      <Player
                        videoId={videoId}
                        onClickClose={() =>
                          setVideoIds((prev) =>
                            prev.filter((id) => id !== videoId),
                          )
                        }
                        LikeButton={
                          /* ログイン直後のみ、likeMap()がundefになってしまう */
                          <Show when={likeMap()} keyed>
                            {(likes) => (
                              <LikeButton
                                liked={likes.get(videoId)}
                                onClick={async () => {
                                  setLiked((prev) => prev.set(videoId, true));
                                  try {
                                    await like(videoId);
                                  } catch {
                                    return setLiked((prev) =>
                                      prev.set(videoId, false),
                                    );
                                  }
                                }}
                              />
                            )}
                          </Show>
                        }
                      />
                    </>
                  )}
                </For>
              </div>
            )}
          </Show>
        </div>
      </div>
    </div>
  );
};
export default Watch;
