import { createMiddleware } from "@solidjs/start/middleware";
import { cloudflare } from "~/middlewares/cloudflare";

export default createMiddleware({
  onRequest: [cloudflare()],
});
