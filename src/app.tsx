import "virtual:uno.css";
import "@unocss/reset/sanitize/sanitize.css";
import "./global.css";

// @refresh reload
import { Router } from "@solidjs/router";
import { HttpHeader, HttpStatusCode } from "@solidjs/start";
import { FileRoutes } from "@solidjs/start/router";
import { ErrorBoundary, Match, Suspense, Switch } from "solid-js";
import { getRequestEvent, isServer } from "solid-js/web";
import { isTokenExpired } from "~/libs/api/youtube";
import { YouTubeApiProvider } from "~/libs/api/youtube/context";
import { getAuthTokens } from "~/libs/session";
import { BareLayout } from "./layouts/bare";

export default function App() {
  return (
    <YouTubeApiProvider
      getAuthTokens={() =>
        getAuthTokens({ secret: process.env.SESSION_SECRET! })
      }
    >
      <BareLayout>
        <ErrorBoundary
          fallback={(err) => {
            console.error(err);
            return (
              <Switch>
                <Match when={isServer && isTokenExpired(err)}>
                  {(_) => {
                    const event = getRequestEvent();
                    const redirectTo = event
                      ? new URL(event.request.url).pathname
                      : "/";
                    return (
                      <>
                        <HttpStatusCode code={302} />
                        <HttpHeader
                          name="Location"
                          value={`/login?redirect_to=${redirectTo}`}
                        />
                      </>
                    );
                  }}
                </Match>
                <Match when={err}>
                  <div>Error: {err.message}</div>
                </Match>
              </Switch>
            );
          }}
        >
          <Router root={(props) => <Suspense> {props.children} </Suspense>}>
            <FileRoutes />
          </Router>
        </ErrorBoundary>
      </BareLayout>
    </YouTubeApiProvider>
  );
}
