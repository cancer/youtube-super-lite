/**
 * declarativeNetRequest の `condition.urlFilter` の照合を模擬する。
 *
 * これは **DNR の挙動そのものではない**。実際の遮断はブラウザが行うため、ここで判定できるのは
 * 「ルールが意図した URL 集合を書けているか」までである。実機での遮断結果の保証にはならない。
 *
 * それでも自前で持つ理由は、要件が「遮断してはならない」と明記した系統
 * （heartbeat / attestation / playback / watchtime）を、ブラウザ抜きで機械的に守りたいこと。
 * ルール追加時の巻き添えは URL 集合の問題なので、この層で検出できる。
 *
 * 対応する構文は urlFilter のみ（`regexFilter` は扱わない）。仕様の出所は
 * https://developer.chrome.com/docs/extensions/reference/api/declarativeNetRequest
 */

/** `^` が一致する「セパレータ」の定義。英数字と `_-.%` 以外、および URL の終端。 */
const SEPARATOR = "(?:[^A-Za-z0-9_\\-.%]|$)";

/**
 * ドメインアンカー `||` の前置。スキームを読み飛ばし、ホストの区切り（`.`）直後にも
 * 照合開始位置を許す。`[^/?#]*` でホスト部から出ないようにしてあるのは、
 * `||google.com/` がクエリ文字列中の `google.com/` に誤って一致するのを防ぐため。
 */
const DOMAIN_ANCHOR = "^[^:]+://(?:[^/?#]*\\.)?";

const escapeLiteral = (char: string): string =>
  /[A-Za-z0-9]/.test(char) ? char : `\\${char}`;

const toRegExpSource = (body: string): string =>
  [...body]
    .map((char) => {
      if (char === "*") return ".*";
      if (char === "^") return SEPARATOR;
      return escapeLiteral(char);
    })
    .join("");

const toRegExp = (urlFilter: string): RegExp => {
  const domainAnchored = urlFilter.startsWith("||");
  const startAnchored = !domainAnchored && urlFilter.startsWith("|");
  const withoutStart = urlFilter.slice(domainAnchored ? 2 : startAnchored ? 1 : 0);

  const endAnchored = withoutStart.endsWith("|");
  const body = endAnchored ? withoutStart.slice(0, -1) : withoutStart;

  const prefix = domainAnchored ? DOMAIN_ANCHOR : startAnchored ? "^" : "";
  // isUrlFilterCaseSensitive の既定は false なので、既定の照合を模擬する側も区別しない。
  return new RegExp(
    `${prefix}${toRegExpSource(body)}${endAnchored ? "$" : ""}`,
    "i",
  );
};

/** `urlFilter` が `url` に一致するか。 */
export const matchesUrlFilter = (urlFilter: string, url: string): boolean =>
  toRegExp(urlFilter).test(url);
