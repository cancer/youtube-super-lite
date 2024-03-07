import "virtual:uno.css";
import "@unocss/reset/sanitize/sanitize.css";
import "./global.css";

// @refresh reload
import { Router } from "@solidjs/router";
import { FileRoutes } from "@solidjs/start";
import { ErrorBoundary, Suspense } from "solid-js";
import { YouTubeApiProvider } from "~/libs/api/youtube/context";
import { getAuthTokens } from "~/libs/session";

export default function App() {
  return (
    <ErrorBoundary
      fallback={(err) => {
        console.log(err);
        return <div>Error: {err.message}</div>;
      }}
    >
      <YouTubeApiProvider
        getAuthTokens={() =>
          getAuthTokens({ secret: process.env.SESSION_SECRET! })
        }
      >
        <Router root={(props) => <Suspense>{props.children}</Suspense>}>
          <FileRoutes />
        </Router>
      </YouTubeApiProvider>
    </ErrorBoundary>
  );
}
