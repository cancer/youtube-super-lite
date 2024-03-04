import { createContext, FlowComponent, useContext } from "solid-js";
import {
  createYouTubeApiClient,
  type YouTubeApiClient,
} from "~/libs/api/youtube/client";

export const youTubeApiContext = createContext<YouTubeApiClient | null>(null);

type Props = { accessToken: string };
export const YouTubeApiProvider: FlowComponent<Props> = (props) => (
  <youTubeApiContext.Provider
    value={createYouTubeApiClient({ accessToken: props.accessToken })}
  >
    {props.children}
  </youTubeApiContext.Provider>
);

export const useYouTubeApiClient = () => {
  const client = useContext(youTubeApiContext);
  if (client === null) throw new Error("<YouTubeApiProvider> is not defined.");
  return client;
};
