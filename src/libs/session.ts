import { useSession } from "vinxi/http";
import { type AuthSessions } from "~/libs/auth-sessions/client";

// @deprecated use AuthTokenClient.get()
export const getAuthTokens = async (args: { secret: string }) => {
  "use server";
  const session = await _getSession<AuthSessions>(args);
  if (!("accessToken" in session.data)) return null;
  return session.data;
};

export const setAuthTokens = async (
  tokens: AuthSessions,
  args: { secret: string },
) => {
  "use server";
  const session = await _getSession<AuthSessions>(args);
  await session.update(() => ({ ...tokens }));
};

export const clearAuthTokens = async (args: { secret: string }) => {
  "use server";
  const session = await _getSession<AuthSessions>(args);
  await session.clear();
};

const _getSession = <T extends Record<string, any>>({
  secret,
}: {
  secret: string;
}) => {
  "use server";
  return useSession<T>({
    name: "ytp_session",
    password: secret!,
  });
};

export type Session = Awaited<ReturnType<typeof useSession>>

export const getSession = (secret: string) => {
  "use server";
  return useSession({
    name: "ytp_session",
    password: secret,
  });
};
