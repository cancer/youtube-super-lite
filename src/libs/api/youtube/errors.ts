export class TokenExpiredError extends Error {
  name = "TokenExpiredError";

  constructor() {
    super("Token has expired.");
  }
}

export const isTokenExpired = (err: unknown): err is TokenExpiredError => {
  if (typeof err !== "object") return false;
  if (!(err instanceof Error)) return false;

  if (err instanceof TokenExpiredError) return true;
  if (err.name === "TokenExpiredError") return true;

  return false;
};
