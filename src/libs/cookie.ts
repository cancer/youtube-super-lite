type Serialize = (
  key: string,
  value: any,
  options: {
    "Max-Age"?: number;
    Domain?: string;
    Path?: string;
    Secure?: boolean;
    HttpOnly?: boolean;
    SameSite?: "Strict" | "Lax" | "None";
  },
) => string;
export const serialize: Serialize = (key, value, options) => {
  const serializedOptions = Object.entries(options).reduce(
    (acc, [key, value]) => {
      if (key === "Secure") return `${acc}; Secure`;
      if (key === "HttpOnly") return `${acc}; HttpOnly`;
      return `${acc}; ${key}=${value}`;
    },
    "",
  );

  return `${encodeURIComponent(key)}=${encodeURIComponent(
    value,
  )}${serializedOptions}`;
};
