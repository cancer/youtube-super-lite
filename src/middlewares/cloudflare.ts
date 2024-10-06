import { type RequestMiddleware } from "@solidjs/start/middleware";
import { type CFProxy, getCloudflareProxy } from "~/libs/cloudflare";

declare global {
  interface RequestEventLocals {
    env: CFProxy["env"];
    ctx: CFProxy["ctx"];
  }
}

export const cloudflare: () => RequestMiddleware = () => async (event) => {
  const cloudflare = await getCloudflareProxy();
  event.locals.env = cloudflare.env;
  event.locals.ctx = cloudflare.ctx;
};
