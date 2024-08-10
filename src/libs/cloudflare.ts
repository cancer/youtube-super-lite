import { getRequestEvent } from "solid-js/web";
import { getPlatformProxy } from "wrangler";

type Env = {
  SESSION_SECRET: string;
  GAUTH_CLIENT_ID: string;
  GAUTH_CLIENT_SECRET: string;
};
export const getCloudflareEnv = async (): Promise<Env> => {
  // for remote
  const context = getRequestEvent()?.nativeEvent.context.cloudflare ?? null;
  if (context !== null) return context.env as Env;
  
  // for local
  return (await getPlatformProxy<Env>()).env;
};
