import { createMiddleware } from "@solidjs/start/middleware";
import { auth } from "~/middlewares/auth";
import { cloudflare } from "~/middlewares/cloudflare";
import { youtubeApi } from "~/middlewares/youtube-api";

export default createMiddleware({
  onRequest: [cloudflare(), auth(), youtubeApi()],
});
