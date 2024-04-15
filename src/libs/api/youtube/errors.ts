export class TokenExpiredError extends Error {
  name = "TokenExpiredError";
  
  constructor() {
    super("Token has expired.");
  }
}

export const isTokenExpired = (err: unknown): err is TokenExpiredError => {
  if (!err) return false;
  if (!(err instanceof TokenExpiredError)) return false;
  if (err.name !== "TokenExpiredError") return false;
  return true;
};
