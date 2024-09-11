import { createMiddleware } from "@solidjs/start/middleware";
import { getCloudflareProxy } from "~/libs/cloudflare";

export default createMiddleware({
  async onRequest(event) {
    const cloudflare = await getCloudflareProxy();
    event.locals.env = cloudflare.env;
    event.locals.ctx = cloudflare.ctx;
  },
});
