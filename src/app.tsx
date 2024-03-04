import "virtual:uno.css";
import "@unocss/reset/sanitize/sanitize.css";
import "./global.css";

// @refresh reload
import { Router } from "@solidjs/router";
import { FileRoutes } from "@solidjs/start";
import { Suspense } from "solid-js";
import { getCookie } from "vinxi/http";
import { YouTubeApiProvider } from "~/libs/api/youtube/context";

const getYouTubeApiAccessToken = () => {
  "use server";
  return getCookie("ytp_tokens") ?? "";
};

export default function App() {
  return (
    <YouTubeApiProvider accessToken={getYouTubeApiAccessToken()}>
      <Router root={(props) => <Suspense>{props.children}</Suspense>}>
        <FileRoutes />
      </Router>
    </YouTubeApiProvider>
  );
}
