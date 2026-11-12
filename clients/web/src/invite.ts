/** 同じ公開入口にある Host の WebSocket を解決する。 */
export function hostWebSocketUrl(pageUrl: string): string {
  const url = new URL(pageUrl);
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error("Web の URL が不正です");
  }
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  url.pathname = "/remote";
  url.search = "";
  url.hash = "";
  return url.toString();
}

/** fragment のみを読み、旧形式や重複パラメータを受け付けない。 */
export function parseInviteFragment(hash: string): { token: string; valid: boolean } | null {
  const prefix = "#/invite";
  if (hash !== prefix && !hash.startsWith(`${prefix}?`)) return null;
  const params = new URLSearchParams(hash.slice(prefix.length + 1));
  const tokens = params.getAll("token");
  const token = (tokens[0] ?? "").trim();
  const valid = tokens.length === 1 && [...params.keys()].every((key) => key === "token") && /^[0-9a-f]{64}$/i.test(token);
  return { token, valid };
}

/** 招待での再登録は失効済み端末と異なる ID を発行する。 */
export function selectClientId(token: string | null, saved: string | null, create: () => string): string {
  return token !== null ? create() : saved ?? create();
}
