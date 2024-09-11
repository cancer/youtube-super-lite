import { getRequestEvent } from "solid-js/web";
import { type PlatformProxy } from "wrangler";

type Env = {
  SESSION_SECRET: string;
  GAUTH_CLIENT_ID: string;
  GAUTH_CLIENT_SECRET: string;
};
export const getCloudflareProxy = async (): Promise<{
  env: Env;
  ctx: PlatformProxy["ctx"];
}> => {
  const proxy = getRequestEvent()?.nativeEvent.context.cloudflare ?? null;
  
  // XXX: import.metaを直接見ないと、ビルド時にこのスコープが解析されてエラーになる
  if (import.meta.env.DEV) {
    // for remote
    if (proxy !== null) return { env: proxy.env as Env, ctx: proxy.context };

    // for local
    // XXX: ビルド時にgetPlatformProxyが解析されるとエラーになるので、実行時にimportする
    return (await import("wrangler"))
      .getPlatformProxy<Env>()
      .then(({ env, ctx }) => ({ env, ctx }));
  }

  if (proxy === null)
    return { env: {} as Env, ctx: {} as PlatformProxy["ctx"] };
  return { env: proxy.env as Env, ctx: proxy.context };
};
