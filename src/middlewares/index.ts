import { createMiddleware } from "@solidjs/start/middleware";
import { auth } from "~/middlewares/auth";
import { cloudflare } from "~/middlewares/cloudflare";

export default createMiddleware({
  onRequest: [cloudflare(), auth()],
});
