import { type AuthTokens } from "~/libs/auth-tokens/types";
import { type Session } from "~/libs/session";

export type AuthTokensClient = {
  get: () => Promise<AuthTokens | null>;
  clear: () => Promise<null>;
};
export const createAuthTokensClient = (
  getSession: () => Promise<Session>,
): AuthTokensClient => {
  "use server";
  return {
    get: async () => {
      const session = await getSession();
      if (!("accessToken" in session.data)) return null;
      return session.data as AuthTokens;
    },
    clear: async () => {
      const session = await getSession();
      await session.clear();
      return null;
    },
  };
};
