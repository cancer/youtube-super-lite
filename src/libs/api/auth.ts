type ApiClient = {
  request: (args: {
    url: string;
    body: Record<string, unknown>;
  }) => Promise<any>;
};
type AuthApi<T> = (client: ApiClient) => T;

export const createAuthClient: (credentials: {
  clientId: string;
  clientSecret: string;
}) => ApiClient = ({ clientId, clientSecret }) => ({
  request: async ({
    url,
    body,
  }: {
    url: string;
    body: Record<string, unknown>;
  }) => {
    const res = await fetch(url, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        ...body,
        client_id: clientId,
        client_secret: clientSecret,
      }),
    });

    if (!res.ok) throw new Error(await res.text());

    return res.json();
  },
});

type Tokens = {
  accessToken: string;
  refreshToken: string;
  expiresIn: number;
};
type ExchangeTokens = (params: {
  code: string;
  redirectUri: string;
}) => Promise<Tokens>;
export const exchangeTokens: AuthApi<ExchangeTokens> =
  (client) =>
  async ({ code, redirectUri }) => {
    const json = await client.request({
      url: "https://oauth2.googleapis.com/token",
      body: {
        code: code,
        redirect_uri: redirectUri,
        grant_type: "authorization_code",
      },
    });

    try {
      return adaptTokensIfValid(json);
    } catch {
      throw new Error(JSON.stringify(json));
    }
  };

const adaptTokensIfValid = (json: unknown): Tokens => {
  const err = new Error("Invalid response");
  if (typeof json !== "object") throw err;
  if (json === null) throw err;
  if (!("access_token" in json) || typeof json.access_token !== "string")
    throw err;
  if (!("refresh_token" in json) || typeof json.refresh_token !== "string")
    throw err;
  if (!("expires_in" in json) || typeof json.expires_in !== "number") throw err;

  return {
    accessToken: json.access_token,
    refreshToken: json.refresh_token,
    expiresIn: json.expires_in,
  };
};
