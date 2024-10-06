import type { RequestMiddleware } from "@solidjs/start/middleware";
import { useSession } from "vinxi/http";
import {
  type AuthSessionsClient,
  createAuthSessionsClient,
} from "~/libs/auth-sessions/client";

declare global {
  interface RequestEventLocals {
    auth: AuthSessionsClient;
  }
}

export const auth: () => RequestMiddleware = () => async (event) => {
  (event.locals as RequestEventLocals).auth = createAuthSessionsClient(() =>
    useSession({
      name: "ytp_session",
      password: event.locals.env.SESSION_SECRET,
    }),
  );
};
