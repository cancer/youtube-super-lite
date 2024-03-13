import "virtual:uno.css";
import "@unocss/reset/sanitize/sanitize.css";
import "./global.css";

// @refresh reload
import { Router } from "@solidjs/router";
import { FileRoutes } from "@solidjs/start";
import { ErrorBoundary, Suspense } from "solid-js";
import { YouTubeApiProvider } from "~/libs/api/youtube/context";
import { getAuthTokens } from "~/libs/session";
import { BaseLayout } from "./layouts/base";

export default function App() {
  return (
    <ErrorBoundary
      fallback={(err) => {
        console.log(err);
        return <div class="text-white">Error: {err.message}</div>;
      }}
    >
      <YouTubeApiProvider
        getAuthTokens={() =>
          getAuthTokens({ secret: process.env.SESSION_SECRET! })
        }
      >
        <Router
          root={(props) => (
            <Suspense>
              <BaseLayout>{props.children}</BaseLayout>
            </Suspense>
          )}
        >
          <FileRoutes />
        </Router>
      </YouTubeApiProvider>
    </ErrorBoundary>
  );
}
