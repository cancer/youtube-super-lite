import { createMiddleware } from "@solidjs/start/middleware";
import { getCloudflareEnv } from "~/libs/cloudflare";

export default createMiddleware({
  async onRequest(event) {
    event.locals.env = await getCloudflareEnv();
  }
})
