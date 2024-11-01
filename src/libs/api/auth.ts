type ApiClient = {
  request: (args: {
    url: string;
    body?: Record<string, unknown>;
    params?: Record<string, unknown>;
    headers?: Record<string, string>;
  }) => Promise<any>;
};
type AuthApi<T> = (client: ApiClient) => T;

type Tokens = {
  accessToken: string;
  refreshToken: string;
  expiresIn: number;
};

export const createAuthApiClient: (credentials: {
  clientId: string;
  clientSecret: string;
}) => ApiClient = ({ clientId, clientSecret }) => {
  return {
    request: async ({ url: _url, body: _body, params, headers }) => {
      const url = new URL(_url);
      for (const [key, value] of Object.entries(params ?? {})) {
        url.searchParams.set(key, String(value));
      }

      let body;
      switch (headers?.["Content-Type"]) {
        case "application/x-www-form-urlencoded": {
          body = new URLSearchParams();
          for (const [key, value] of Object.entries(_body ?? {})) {
            body.append(key, String(value));
          }
          break;
        }
        case "application/json":
        default: {
          body = JSON.stringify({
            ..._body,
            client_id: clientId,
            client_secret: clientSecret,
          });
          break;
        }
      }

      const res = await fetch(url, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          ...headers,
        },
        body,
      });

      if (!res.ok) throw new Error(await res.text());

      return res.json();
    },
  };
};

type ExchangeTokens = AuthApi<
  (params: { code: string; redirectUri: string }) => Promise<Tokens>
>;
export const exchangeTokens: ExchangeTokens = (client) => {
  "use server";
  return async ({ code, redirectUri }) => {
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
};

type RefreshAccessToken = AuthApi<(refreshToken: string) => Promise<Tokens>>;
export const refreshAccessToken: RefreshAccessToken = (client) => {
  "use server";
  return async (refreshToken) => {
    "use server";
    const json = await client.request({
      url: "https://oauth2.googleapis.com/token",
      body: {
        grant_type: "refresh_token",
        refresh_token: refreshToken,
      },
    });

    try {
      return adaptTokensIfValid(json);
    } catch {
      throw new Error(JSON.stringify(json));
    }
  };
};

type RevokeToken = AuthApi<(token: string) => Promise<null>>;
export const revokeToken: RevokeToken = (client) => {
  "use server";
  return async (token) => {
    "use server";
    await client.request({
      url: "https://oauth2.googleapis.com/revoke",
      headers: {
        "Content-Type": "application/x-www-form-urlencoded",
      },
      body: { token },
    });

    return null;
  };
};

const adaptTokensIfValid = (json: unknown): Tokens => {
  if (typeof json !== "object") throw new Error("Invalid response");
  if (json === null) throw new Error("Invalid response");
  if (!("access_token" in json) || typeof json.access_token !== "string")
    throw new Error("Invalid response. access_token does not exist.");
  if (!("expires_in" in json) || typeof json.expires_in !== "number")
    throw new Error("Invalid response. expires_in does not exist.");

  return {
    accessToken: json.access_token,
    refreshToken:
      "refresh_token" in json && typeof json.refresh_token === "string"
        ? json.refresh_token
        : "",
    expiresIn: json.expires_in,
  };
};
