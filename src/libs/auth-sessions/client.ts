import { type Session } from "~/libs/session";

export type AuthSessions = {
  accessToken: string;
  refreshToken: string;
  expiresAt: number;
};

export type AuthSessionsClient = {
  get: () => Promise<AuthSessions | null>;
  clear: () => Promise<null>;
};
export const createAuthSessionsClient = (
  getSession: () => Promise<Session>,
): AuthSessionsClient => {
  "use server";
  return {
    get: async () => {
      const session = await getSession();
      if (!("accessToken" in session.data)) return null;
      return session.data as AuthSessions;
    },
    clear: async () => {
      const session = await getSession();
      await session.clear();
      return null;
    },
  };
};
