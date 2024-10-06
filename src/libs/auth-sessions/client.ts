type Session = {
  // biome-disable-next-line no-explicit-any
  data: Record<string, any>;
  update: (
    // biome-disable-next-line no-explicit-any
    callback: (data: Record<string, any>) => Record<string, any>,
  ) => Promise<void>;
  clear: () => Promise<void>;
};

export type AuthSession = {
  accessToken: string;
  refreshToken: string;
  expiresAt: number;
};

export type AuthSessionsClient = {
  get: () => Promise<AuthSession | null>;
  set: (values: AuthSession) => Promise<null>;
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
      return session.data as AuthSession;
    },
    set: async (values) => {
      const session = await getSession();
      await session.update(() => ({ ...values }));
      return null;
    },
    clear: async () => {
      const session = await getSession();
      await session.clear();
      return null;
    },
  };
};
