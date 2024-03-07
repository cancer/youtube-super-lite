import { createContext, FlowComponent, useContext } from "solid-js";
import {
  createYouTubeApiClient,
  type YouTubeApiClient,
} from "~/libs/api/youtube/client";
import { AuthTokens } from "~/libs/session";

export const youTubeApiContext = createContext<YouTubeApiClient | null>(null);

type Props = {
  getAuthTokens: () => Promise<AuthTokens | null>;
};
export const YouTubeApiProvider: FlowComponent<Props> = (props) => (
  <youTubeApiContext.Provider
    value={createYouTubeApiClient({ getAuthTokens: props.getAuthTokens })}
  >
    {props.children}
  </youTubeApiContext.Provider>
);

export const useYouTubeApiClient = () => {
  const client = useContext(youTubeApiContext);
  if (client === null) throw new Error("<YouTubeApiProvider> is not defined.");
  return client;
};
