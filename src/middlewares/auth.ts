import { type RequestMiddleware } from "@solidjs/start/middleware";
import {
  type AuthSessionsClient,
  createAuthSessionsClient,
} from "~/libs/auth-sessions/client";
import { getSession } from "~/libs/session";

declare global {
  interface RequestEventLocals {
    auth: AuthSessionsClient;
  }
}

export const auth: () => RequestMiddleware = () => async (event) => {
  (event.locals as RequestEventLocals).auth = createAuthSessionsClient(() =>
    getSession(event.locals.env.SESSION_SECRET),
  );
};
