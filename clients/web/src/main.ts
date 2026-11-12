import init, { connectionInfo, is_ready, start } from "./generated/wasm/lapis_ui.js";
import { hostWebSocketUrl, parseInviteFragment, selectClientId } from "./invite.js";

const root = document.querySelector<HTMLElement>("#app");
if (root === null) {
  throw new Error("Lapis Web root was not found");
}

root.innerHTML = `<section class="loading" role="status"><strong>Lapis Web</strong><span>Desktop UI を読み込んでいます…</span></section>`;

const APP_NAME = "Lapis";
const STORAGE = {
  url: "lapis.url",
  workspace: "lapis.workspace",
  clientId: "lapis.clientId",
  registry: "lapis.registry.v1",
  pendingFragment: "lapis.pendingFragment",
};
const LEGACY_WARNING =
  "旧形式の URL に含まれる接続先・token は使用しません。接続先を選ぶか、保存済みの接続から開き直してください。";
// ????? Workspace ????????? registry ??????
let connectedUrl: string | null = null;

type ConnectionInfo = {
  host_name: string;
  host_id?: string | null;
  workspace_name: string;
  workspace_id: string;
  can_switch: boolean;
  workspaces: Array<{ id: string; name: string }>;
};

type WorkspaceChoice = { id: string; name: string };

type SavedHost = {
  url: string;
  host_name: string;
  host_id?: string | null;
  workspaces: WorkspaceChoice[];
  updated_at: number;
};

type AliasEntry = {
  kind: "host" | "workspace";
  host_url: string;
  workspace_id?: string;
  label: string;
  created_at: number;
};

type PathSelection = {
  path: string;
  host_url: string;
  workspace_id: string;
};

type Registry = {
  hosts: SavedHost[];
  aliases: AliasEntry[];
  selections: PathSelection[];
};

type Candidate = {
  host: SavedHost;
  workspace: WorkspaceChoice;
};

type FragmentState = {
  taskId?: string;
  invite?: { token: string; valid: boolean };
};

type ResolvedPath =
  | { kind: "resolved"; candidate: Candidate }
  | { kind: "ambiguous"; candidates: Candidate[] }
  | { kind: "unresolved"; candidates: Candidate[] };

function createClientId(): string {
  if (typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
  return [hex.slice(0, 8), hex.slice(8, 12), hex.slice(12, 16), hex.slice(16, 20), hex.slice(20)].join("-");
}

function escapeHtml(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function readLocal(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeLocal(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // 保存領域へアクセスできない場合は、その場の接続だけを続ける。
  }
}

function readSession(key: string): string | null {
  try {
    return sessionStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeSession(key: string, value: string) {
  try {
    sessionStorage.setItem(key, value);
  } catch {
    // 保存領域へアクセスできない場合は、その場の接続だけを続ける。
  }
}

function removeSession(key: string) {
  try {
    sessionStorage.removeItem(key);
  } catch {
    // 保存領域へアクセスできない場合は何もしない。
  }
}

function normalizeLabel(value: string): string {
  return value.trim().normalize("NFC").toLocaleLowerCase();
}

function decodePathSegment(part: string): string {
  let value: string;
  try {
    value = decodeURIComponent(part);
  } catch {
    return part;
  }
  if (!value.startsWith("~")) {
    return value;
  }
  const hex = value.slice(1);
  if (hex === "") {
    return "";
  }
  if (hex.length % 2 !== 0 || !/^[0-9a-f]+$/i.test(hex)) {
    return value;
  }
  const pairs = hex.match(/../g);
  if (pairs === null) {
    return value;
  }
  try {
    return new TextDecoder().decode(
      new Uint8Array(pairs.map((pair) => Number.parseInt(pair, 16))),
    );
  } catch {
    return value;
  }
}

function encodePathSegment(segment: string): string {
  const value = segment.trim();
  // 空・dot-only・`~` 始まりは、URL の正規化で別のパスにならないよう hex 退避する。
  if (value !== "" && value !== "." && value !== ".." && !value.startsWith("~")) {
    return encodeURIComponent(value);
  }
  const bytes = new TextEncoder().encode(value);
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
  return `~${hex}`;
}

function splitPath(): string[] {
  const parts = location.pathname.split("/").filter((part) => part !== "");
  return parts.map(decodePathSegment);
}

function pathFromSegments(segments: string[]): string {
  return `/${segments.map(encodePathSegment).join("/")}`;
}

function pathFromCandidate(candidate: Candidate): string {
  return pathFromSegments([candidate.host.host_name, candidate.workspace.name]);
}

function normalizePathKey(path: string): string {
  return path.replace(/\/+$/, "") || "/";
}

function emptyRegistry(): Registry {
  return { hosts: [], aliases: [], selections: [] };
}

function isWorkspaceChoice(value: unknown): value is WorkspaceChoice {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const record = value as Record<string, unknown>;
  return typeof record.id === "string" && typeof record.name === "string";
}

function isSavedHost(value: unknown): value is SavedHost {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const record = value as Record<string, unknown>;
  return (
    typeof record.url === "string" &&
    typeof record.host_name === "string" &&
    Array.isArray(record.workspaces) &&
    record.workspaces.every(isWorkspaceChoice)
  );
}

function isAlias(value: unknown): value is AliasEntry {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const record = value as Record<string, unknown>;
  return (
    (record.kind === "host" || record.kind === "workspace") &&
    typeof record.host_url === "string" &&
    typeof record.label === "string"
  );
}

function isSelection(value: unknown): value is PathSelection {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const record = value as Record<string, unknown>;
  return (
    typeof record.path === "string" &&
    typeof record.host_url === "string" &&
    typeof record.workspace_id === "string"
  );
}

function loadRegistry(): Registry {
  const raw = readLocal(STORAGE.registry);
  if (raw === null) {
    return emptyRegistry();
  }
  try {
    const value = JSON.parse(raw) as Partial<Registry>;
    return {
      hosts: Array.isArray(value.hosts) ? value.hosts.filter(isSavedHost) : [],
      aliases: Array.isArray(value.aliases) ? value.aliases.filter(isAlias) : [],
      selections: Array.isArray(value.selections) ? value.selections.filter(isSelection) : [],
    };
  } catch {
    return emptyRegistry();
  }
}

function saveRegistry(registry: Registry) {
  writeLocal(STORAGE.registry, JSON.stringify(registry));
}

// 改名時の alias をいつまで解決するか。期間の値は未決定のため、現状は無期限。
const ALIAS_RETENTION_MS: number | null = null;

function pruneAliases(registry: Registry) {
  if (ALIAS_RETENTION_MS === null) {
    return;
  }
  const cutoff = Date.now() - ALIAS_RETENTION_MS;
  registry.aliases = registry.aliases.filter((alias) => alias.created_at >= cutoff);
}

function addAlias(
  registry: Registry,
  kind: AliasEntry["kind"],
  hostUrl: string,
  workspaceId: string | undefined,
  label: string,
) {
  const trimmed = label.trim();
  if (trimmed === "") {
    return;
  }
  const exists = registry.aliases.some(
    (alias) =>
      alias.kind === kind &&
      alias.host_url === hostUrl &&
      alias.workspace_id === workspaceId &&
      normalizeLabel(alias.label) === normalizeLabel(trimmed),
  );
  if (!exists) {
    registry.aliases.push({
      kind,
      host_url: hostUrl,
      workspace_id: workspaceId,
      label: trimmed,
      created_at: Date.now(),
    });
  }
}

function rememberSelection(registry: Registry, path: string, hostUrl: string, workspaceId: string) {
  const key = normalizePathKey(path);
  registry.selections = registry.selections.filter((selection) => selection.path !== key);
  registry.selections.push({ path: key, host_url: hostUrl, workspace_id: workspaceId });
}

function workspaceChoices(info: ConnectionInfo): WorkspaceChoice[] {
  const choices = [...info.workspaces];
  if (!choices.some((workspace) => workspace.id === info.workspace_id)) {
    choices.push({ id: info.workspace_id, name: info.workspace_name });
  }
  const unique = new Map<string, WorkspaceChoice>();
  for (const choice of choices) {
    if (!unique.has(choice.id)) {
      unique.set(choice.id, { id: choice.id, name: choice.name });
    }
  }
  return [...unique.values()];
}

function saveConnection(info: ConnectionInfo, url: string): { path: string; registry: Registry } {
  const registry = loadRegistry();
  const previous = registry.hosts.find(
    (host) =>
      (info.host_id !== null &&
        info.host_id !== undefined &&
        host.host_id === info.host_id) ||
      host.url === url,
  );
  if (previous !== undefined) {
    if (
      previous.host_name.trim() !== "" &&
      info.host_name.trim() !== "" &&
      normalizeLabel(previous.host_name) !== normalizeLabel(info.host_name)
    ) {
      addAlias(registry, "host", url, undefined, previous.host_name);
    }
    for (const workspace of previous.workspaces) {
      const current = info.workspaces.find((choice) => choice.id === workspace.id);
      if (
        current !== undefined &&
        workspace.name.trim() !== "" &&
        current.name.trim() !== "" &&
        normalizeLabel(workspace.name) !== normalizeLabel(current.name)
      ) {
        addAlias(registry, "workspace", url, workspace.id, workspace.name);
      }
    }
  }
  pruneAliases(registry);
  const entry: SavedHost = {
    url,
    host_name: info.host_name,
    host_id: info.host_id,
    workspaces: workspaceChoices(info),
    updated_at: Date.now(),
  };
  registry.hosts = registry.hosts.filter(
    (host) =>
      host.url !== url &&
      !(
        entry.host_id !== null &&
        entry.host_id !== undefined &&
        host.host_id === entry.host_id
      ),
  );
  registry.hosts.push(entry);
  saveRegistry(registry);
  writeLocal(STORAGE.url, url);
  writeLocal(STORAGE.workspace, info.workspace_id);
  return {
    path: pathFromSegments([info.host_name, info.workspace_name]),
    registry,
  };
}

function lastHost(registry: Registry): SavedHost | undefined {
  return [...registry.hosts].sort((left, right) => right.updated_at - left.updated_at)[0];
}

function allCandidates(registry: Registry): Candidate[] {
  return registry.hosts.flatMap((host) =>
    host.workspaces.map((workspace) => ({ host, workspace })),
  );
}

function resolveSavedPath(segments: string[], registry: Registry): ResolvedPath {
  const path = normalizePathKey(pathFromSegments(segments));
  const selection = registry.selections.find((entry) => entry.path === path);
  if (selection !== undefined) {
    const host = registry.hosts.find((candidate) => candidate.url === selection.host_url);
    const workspace = host?.workspaces.find(
      (candidate) => candidate.id === selection.workspace_id,
    );
    if (host !== undefined && workspace !== undefined) {
      return { kind: "resolved", candidate: { host, workspace } };
    }
  }
  const hostLabel = normalizeLabel(segments[0] ?? "");
  const workspaceLabel = normalizeLabel(segments[1] ?? "");
  const candidates = allCandidates(registry).filter((candidate) => {
    const hostMatch =
      normalizeLabel(candidate.host.host_name) === hostLabel ||
      registry.aliases.some(
        (alias) =>
          alias.kind === "host" &&
          alias.host_url === candidate.host.url &&
          normalizeLabel(alias.label) === hostLabel,
      );
    const workspaceMatch =
      normalizeLabel(candidate.workspace.name) === workspaceLabel ||
      registry.aliases.some(
        (alias) =>
          alias.kind === "workspace" &&
          alias.host_url === candidate.host.url &&
          alias.workspace_id === candidate.workspace.id &&
          normalizeLabel(alias.label) === workspaceLabel,
      );
    return hostMatch && workspaceMatch;
  });
  const unique = new Map<string, Candidate>();
  for (const candidate of candidates) {
    unique.set(`${candidate.host.url}\n${candidate.workspace.id}`, candidate);
  }
  const values = [...unique.values()];
  if (values.length === 1) {
    return { kind: "resolved", candidate: values[0] };
  }
  if (values.length > 1) {
    return { kind: "ambiguous", candidates: values };
  }
  return { kind: "unresolved", candidates: allCandidates(registry) };
}

function shortWorkspaceLabel(info: ConnectionInfo): string {
  const name = info.workspace_name.trim().replace(/\s+/g, " ");
  if (name === "" || name === info.workspace_id) {
    return "Workspace";
  }
  return name;
}

function windowTitle(info: ConnectionInfo): string {
  return `${shortWorkspaceLabel(info)} - ${APP_NAME}`;
}

function readFragment(): FragmentState | null {
  const hash = location.hash;
  if (hash.startsWith("#/t/")) {
    const id = hash.slice(4);
    if (id === "") {
      return null;
    }
    try {
      return { taskId: decodeURIComponent(id) };
    } catch {
      return { taskId: id };
    }
  }
  const invite = parseInviteFragment(hash);
  if (invite !== null) return { invite };
  return null;
}

function currentTaskFragment(): string | null {
  const fragment = readFragment();
  return fragment?.taskId ?? null;
}

function setTaskFragment(taskId: string | null) {
  const base = location.pathname + location.search;
  history.replaceState(
    null,
    "",
    taskId === null ? base : `${base}#/t/${encodeURIComponent(taskId)}`,
  );
}

function stashTaskFragment(taskId: string | null) {
  if (taskId !== null) {
    writeSession(
      STORAGE.pendingFragment,
      JSON.stringify({ path: normalizePathKey(location.pathname), taskId }),
    );
  }
}

function takeStashedTaskFragment(): string | null {
  const value = readSession(STORAGE.pendingFragment);
  if (value === null) {
    return null;
  }
  removeSession(STORAGE.pendingFragment);
  try {
    const saved = JSON.parse(value) as { path?: unknown; taskId?: unknown };
    return saved.path === normalizePathKey(location.pathname) && typeof saved.taskId === "string"
      ? saved.taskId
      : null;
  } catch {
    return null;
  }
}

function clearStashedTaskFragment() {
  removeSession(STORAGE.pendingFragment);
}

function clearFragment() {
  history.replaceState(null, "", location.pathname + location.search);
}

function stripLegacyQuery(): boolean {
  const params = new URLSearchParams(location.search);
  const legacy =
    params.has("url") || params.has("workspace") || params.has("token");
  if (!legacy) {
    return false;
  }
  params.delete("url");
  params.delete("workspace");
  params.delete("token");
  const search = params.toString();
  history.replaceState(
    null,
    "",
    location.pathname + (search === "" ? "" : `?${search}`) + location.hash,
  );
  return true;
}

function fail(error: unknown) {
  root!.hidden = false;
  root!.classList.add("error");
  root!.textContent = `起動できません: ${String(error)}。WebGPU/WebGL2 が有効なブラウザーで開いてください。`;
  document.body.dataset.lapis = "error";
}

function showLoginError(error: unknown) {
  const element = document.querySelector<HTMLElement>("#login-error");
  if (element === null) {
    return;
  }
  element.hidden = false;
  element.textContent = `接続できません: ${String(error)}`;
}

function setLoginBusy(busy: boolean) {
  const form = document.querySelector<HTMLFormElement>("#login");
  if (form === null) {
    return;
  }
  for (const element of form.elements) {
    if (element instanceof HTMLInputElement || element instanceof HTMLButtonElement) {
      element.disabled = busy;
    }
  }
  const button = form.querySelector<HTMLButtonElement>("button[type=submit]");
  if (button !== null) {
    button.textContent = busy ? "接続中…" : "接続";
  }
}

type ConnectionInfoPayload = ConnectionInfo | null;

function readConnectionInfo(): ConnectionInfoPayload {
  const raw = connectionInfo();
  if (raw === undefined || raw === null) {
    return null;
  }
  try {
    return JSON.parse(raw) as ConnectionInfo;
  } catch {
    return null;
  }
}

async function launch(
  backend: string,
  url: string | null,
  workspace: string | null,
  token: string | null,
  idToken: string | null,
  taskId: string | null,
) {
  // 新しい招待は端末の再登録でも使う。失敗時に既存 credential の ID を壊さない。
  const clientId = selectClientId(token, localStorage.getItem(STORAGE.clientId), createClientId);
  // 切り替え可能な credential は Workspace に固定されないため、Host 単位で保存する。
  const credentialKey = url ?? "";
  const credential = credentialKey === "" || token !== null ? null : localStorage.getItem(credentialKey);
  if (url !== null) {
    localStorage.setItem(STORAGE.url, url);
    localStorage.setItem(STORAGE.workspace, workspace ?? "");
  }

  await init();
  const fresh = await start(backend, url, workspace, token, credential, idToken, clientId);
  if (token !== null && (fresh === undefined || fresh === null || fresh === "")) {
    throw new Error("招待から credential を保存できませんでした");
  }
  if (fresh !== undefined && fresh !== null && credentialKey !== "") {
    localStorage.setItem(credentialKey, fresh);
  }
  localStorage.setItem(STORAGE.clientId, clientId);
  if (token !== null && url !== null) clearOidcToken(url);

  const deadline = performance.now() + 20_000;
  while (!is_ready()) {
    if (document.body.dataset.lapis === "error") {
      return;
    }
    if (performance.now() > deadline) {
      throw new Error("描画の初期化がtimeoutしました");
    }
    await new Promise(requestAnimationFrame);
  }
  root!.hidden = true;
  document.body.dataset.lapis = "ready";
  if (url === null) {
    document.title = APP_NAME;
    return;
  }
  connectedUrl = url;
  const info = readConnectionInfo();
  if (info !== null) {
    applyConnectedInfo(url, info, taskId);
  }
}

function applyConnectedInfo(url: string, info: ConnectionInfo, taskId: string | null) {
  const { path, registry } = saveConnection(info, url);
  rememberSelection(registry, path, url, info.workspace_id);
  saveRegistry(registry);
  clearStashedTaskFragment();
  history.replaceState(
    null,
    "",
    `${path}${location.search}${taskId === null ? "" : `#/t/${encodeURIComponent(taskId)}`}`,
  );
  document.title = windowTitle(info);
}

// アプリ内で Workspace を切り替えたとき、ページを遷移せず URL と保存済み選択を追従させる。
function applySwitchedInfo(info: ConnectionInfo) {
  if (connectedUrl === null) {
    return;
  }
  const { path, registry } = saveConnection(info, connectedUrl);
  rememberSelection(registry, path, connectedUrl, info.workspace_id);
  saveRegistry(registry);
  // Task の fragment は切り替え後もそのまま残す。
  history.replaceState(null, "", `${path}${location.search}${location.hash}`);
  document.title = windowTitle(info);
}

async function connect(
  url: string,
  workspace: string,
  token: string,
  backend: string,
  taskId: string | null,
) {
  if (url !== hostWebSocketUrl(location.href)) {
    throw new Error("以前の接続先は使用できません。新しい招待リンクを開いてください");
  }
  stashTaskFragment(taskId);
  const idToken = token === "" ? readOidcToken(url) : null;
  try {
    await launch(backend, url, workspace, token === "" ? null : token, idToken, taskId);
  } catch (error) {
    if (idToken === null) {
      throw error;
    }
    clearOidcToken(url);
    if (token === "" && !hasCredential(url)) {
      throw error;
    }
    await launch(backend, url, workspace, token === "" ? null : token, null, taskId);
  }
}

async function launchDemo(backend: string) {
  await launch(backend, null, null, null, null, null);
}

function bridgeOrigin(hostUrl: string): string | null {
  try {
    const url = new URL(hostUrl);
    if (url.protocol === "ws:") {
      url.protocol = "http:";
    } else if (url.protocol === "wss:") {
      url.protocol = "https:";
    } else if (url.protocol !== "http:" && url.protocol !== "https:") {
      return null;
    }
    return url.origin;
  } catch {
    return null;
  }
}

function oidcStorageKey(hostUrl: string): string | null {
  const origin = bridgeOrigin(hostUrl);
  return origin === null ? null : `lapis.oidc.${origin}`;
}

function readOidcToken(hostUrl: string): string | null {
  const key = oidcStorageKey(hostUrl);
  if (key === null) {
    return null;
  }
  return readSession(key);
}

function writeOidcToken(hostUrl: string, token: string): boolean {
  const key = oidcStorageKey(hostUrl);
  if (key === null) {
    return false;
  }
  try {
    sessionStorage.setItem(key, token);
    return true;
  } catch {
    return false;
  }
}

function clearOidcToken(hostUrl: string) {
  const key = oidcStorageKey(hostUrl);
  if (key === null) {
    return;
  }
  removeSession(key);
}

function hasCredential(url: string): boolean {
  return readLocal(url) !== null;
}

function defaultLoginUrl(registry: Registry): string {
  return lastHost(registry)?.url ?? readLocal(STORAGE.url) ?? hostWebSocketUrl(location.href);
}

function defaultLoginWorkspace(registry: Registry): string {
  return readLocal(STORAGE.workspace) ?? lastHost(registry)?.workspaces[0]?.id ?? "";
}

function noticeHtml(notice: string | null): string {
  return notice === null ? "" : `<p class="login-notice">${escapeHtml(notice)}</p>`;
}

function showLogin(
  _defaultUrl: string,
  _defaultWorkspace: string,
  notice: string | null,
  taskId: string | null,
  error?: unknown,
) {
  stashTaskFragment(taskId);
  root!.classList.remove("error");
  root!.hidden = false;
  root!.innerHTML = `
    <form class="login" id="login">
      <h1>Lapis Web</h1>
      <p class="login-lead">Desktop の設定で発行した招待リンクを開くか、ここに貼り付けてください。</p>
      ${noticeHtml(notice)}
      <label>
        <span>招待リンク</span>
        <input name="invite" type="url" required autocomplete="off" placeholder="http://.../#/invite?token=..." />
      </label>
      <button type="submit">接続</button>
      <button type="button" class="secondary" id="oidc-login">OIDC でログイン</button>
      <p class="login-error" id="login-error" hidden></p>
      <a class="login-demo" href="/">Workspace 一覧に戻る</a>
    </form>
  `;

  const form = document.querySelector<HTMLFormElement>("#login");
  if (form === null) {
    return;
  }
  if (error !== undefined) {
    showLoginError(error);
  }

  const backend = () => new URLSearchParams(location.search).get("backend") ?? "auto";

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const value = String(new FormData(form).get("invite") ?? "").trim();
    let invitation: URL;
    try {
      invitation = new URL(value);
    } catch {
      showLoginError("招待リンクを入力してください");
      return;
    }
    if (!["http:", "https:"].includes(invitation.protocol) || !parseInviteFragment(invitation.hash)?.valid) {
      showLoginError("招待リンクの形式が正しくありません");
      return;
    }
    location.assign(invitation.href);
  });

  form.querySelector<HTMLButtonElement>("#oidc-login")?.addEventListener("click", () => {
    const url = hostWebSocketUrl(location.href);
    const workspace = "";
    const token = "";
    const origin = bridgeOrigin(url);
    if (origin === null) {
      showLoginError("Host URL が不正です");
      return;
    }
    const popup = window.open(
      `${origin}/auth/oidc/start`,
      "lapis-oidc",
      "popup,width=520,height=680",
    );
    if (popup === null) {
      showLoginError("ポップアップがブロックされました。ブラウザーの設定を確認してください");
      return;
    }
    setLoginBusy(true);

    let settled = false;
    let closedTimer = 0;
    const cleanup = () => {
      window.removeEventListener("message", onMessage);
      window.clearInterval(closedTimer);
    };
    function onMessage(event: MessageEvent) {
      if (event.origin !== origin || event.source !== popup) {
        return;
      }
      const data = event.data as { type?: unknown; idToken?: unknown } | null;
      if (
        data === null ||
        typeof data !== "object" ||
        data.type !== "lapis.oidc" ||
        typeof data.idToken !== "string" ||
        data.idToken === ""
      ) {
        return;
      }
      settled = true;
      cleanup();
      if (!writeOidcToken(url, data.idToken)) {
        setLoginBusy(false);
        showLoginError("ID Token を保存できません。ブラウザーの保存領域を確認してください");
        try {
          popup?.close();
        } catch {
          // 既に閉じている場合は何もしない。
        }
        return;
      }
      void connect(url, workspace, token, backend(), taskId).catch((connectError) => {
        setLoginBusy(false);
        showLoginError(connectError);
      });
    }
    closedTimer = window.setInterval(() => {
      if (popup.closed && !settled) {
        settled = true;
        cleanup();
        setLoginBusy(false);
        showLoginError("ログインがキャンセルされました");
      }
    }, 500);
    window.addEventListener("message", onMessage);
  });
}

function candidateMarkup(candidate: Candidate, index: number): string {
  const path = pathFromCandidate(candidate);
  const hostName = candidate.host.host_name.trim() || "Host";
  const workspaceName = candidate.workspace.name.trim() || "Workspace";
  return `
    <li>
      <a href="${escapeHtml(path)}" class="workspace-link"
        data-url="${escapeHtml(candidate.host.url)}"
        data-workspace="${escapeHtml(candidate.workspace.id)}"
        data-index="${index}">
        <span>${escapeHtml(workspaceName)}</span>
        <small>${escapeHtml(hostName)}</small>
      </a>
    </li>
  `;
}

function attachCandidateLinks(
  taskId: string | null,
  notice: string | null,
) {
  const backend = new URLSearchParams(location.search).get("backend") ?? "auto";
  for (const link of root!.querySelectorAll<HTMLAnchorElement>(".workspace-link")) {
    link.addEventListener("click", (event) => {
      if (
        event.defaultPrevented ||
        event.button !== 0 ||
        event.metaKey ||
        event.ctrlKey ||
        event.shiftKey ||
        event.altKey
      ) {
        return;
      }
      const url = link.dataset.url;
      const workspace = link.dataset.workspace;
      if (url === undefined || workspace === undefined) {
        return;
      }
      event.preventDefault();
      void connect(url, workspace, "", backend, taskId).catch((error) => {
        showLogin(url, workspace, notice, taskId, error);
      });
    });
  }
}

function showWorkspaceList(
  registry: Registry,
  notice: string | null,
  taskId: string | null,
) {
  clearStashedTaskFragment();
  const candidates = allCandidates(registry);
  if (candidates.length === 0) {
    showLogin(defaultLoginUrl(registry), defaultLoginWorkspace(registry), notice, taskId);
    return;
  }
  root!.classList.remove("error");
  root!.hidden = false;
  root!.innerHTML = `
    <section class="workspace-list">
      <h1>Lapis Web</h1>
      <p class="login-lead">開く Workspace を選びます。</p>
      ${noticeHtml(notice)}
      <ul class="workspace-groups">
        ${registry.hosts
          .map(
            (host) => `
          <li class="workspace-group">
            <strong>${escapeHtml(host.host_name.trim() || "Host")}</strong>
            <ul>
              ${host.workspaces.map((workspace, index) => candidateMarkup({ host, workspace }, index)).join("")}
            </ul>
            <button type="button" class="secondary forget-host" data-url="${escapeHtml(host.url)}">このブラウザーから接続情報を削除</button>
          </li>
        `,
          )
          .join("")}
      </ul>
      <button type="button" class="secondary" id="show-login">別の Host に接続…</button>
    </section>
  `;
  attachCandidateLinks(taskId, notice);
  for (const button of root!.querySelectorAll<HTMLButtonElement>(".forget-host")) {
    button.addEventListener("click", () => {
      const url = button.dataset.url;
      if (url === undefined) return;
      try {
        localStorage.removeItem(url);
        const oidcKey = oidcStorageKey(url);
        if (oidcKey !== null) sessionStorage.removeItem(oidcKey);
        if (localStorage.getItem(STORAGE.url) === url) {
          localStorage.removeItem(STORAGE.url);
          localStorage.removeItem(STORAGE.workspace);
        }
        const updated: Registry = {
          hosts: registry.hosts.filter((host) => host.url !== url),
          aliases: registry.aliases.filter((alias) => alias.host_url !== url),
          selections: registry.selections.filter((selection) => selection.host_url !== url),
        };
        localStorage.setItem(STORAGE.registry, JSON.stringify(updated));
        showWorkspaceList(updated, "このブラウザーの接続情報を削除しました。Host の端末失効は Desktop で行えます。", null);
      } catch {
        showWorkspaceList(loadRegistry(), "接続情報を削除できません。ブラウザーの保存領域を確認してください。", null);
      }
    });
  }
  root!.querySelector<HTMLButtonElement>("#show-login")?.addEventListener("click", () => {
    showLogin(defaultLoginUrl(registry), defaultLoginWorkspace(registry), notice, taskId);
  });
}

function showPicker(
  hostLabel: string,
  workspaceLabel: string,
  candidates: Candidate[],
  notice: string | null,
  taskId: string | null,
) {
  const reason =
    notice ??
    `「${hostLabel} / ${workspaceLabel}」は一意に解決できません。開く Workspace を選んでください。`;
  root!.classList.remove("error");
  root!.hidden = false;
  root!.innerHTML = `
    <section class="workspace-list">
      <h1>Workspace を選択</h1>
      ${noticeHtml(reason)}
      <ul class="workspace-groups">
        ${candidates
          .map((candidate, index) => candidateMarkup(candidate, index))
          .join("")}
      </ul>
      <a class="login-demo" href="/">Workspace 一覧に戻る</a>
    </section>
  `;
  attachCandidateLinks(taskId, reason);
}

async function boot() {
  try {
    const legacy = stripLegacyQuery();
    const params = new URLSearchParams(location.search);
    const backend = params.get("backend") ?? "auto";
    const fragment = readFragment();
    const segments = splitPath();
    if (fragment?.invite !== undefined) {
      const invite = fragment.invite;
      clearFragment();
      const url = hostWebSocketUrl(location.href);
      if (!invite.valid) {
        showLogin(url, "", "招待リンクの形式が正しくありません。Desktop から新しい招待を発行してください。", null);
        return;
      }
      try {
        await connect(url, "", invite.token, backend, null);
      } catch (error) {
        showLogin(url, "", "招待リンクで接続できませんでした。token は URL から削除済みです。", null, error);
      }
      return;
    }

    let taskId = fragment?.taskId ?? null;
    if (taskId === null && segments.length === 2) {
      taskId = takeStashedTaskFragment();
      if (taskId !== null) {
        setTaskFragment(taskId);
      }
    }

    if (params.get("demo") === "1") {
      await launchDemo(backend);
      return;
    }
    const registry = loadRegistry();
    if (segments.length === 0) {
      showWorkspaceList(registry, legacy ? LEGACY_WARNING : null, taskId);
      return;
    }
    if (segments.length !== 2) {
      showWorkspaceList(
        registry,
        legacy ? LEGACY_WARNING : "URL の形式を解釈できません。Workspace を選び直してください。",
        taskId,
      );
      return;
    }
    const resolved = resolveSavedPath(segments, registry);
    if (resolved.kind === "resolved") {
      const { host, workspace } = resolved.candidate;
      if (!hasCredential(host.url) && readOidcToken(host.url) === null) {
        showLogin(host.url, workspace.id, legacy ? LEGACY_WARNING : null, taskId);
        return;
      }
      try {
        await connect(host.url, workspace.id, "", backend, taskId);
      } catch (error) {
        showLogin(host.url, workspace.id, legacy ? LEGACY_WARNING : null, taskId, error);
      }
      return;
    }
    if (resolved.kind === "ambiguous") {
      showPicker(
        segments[0] ?? "",
        segments[1] ?? "",
        resolved.candidates,
        legacy ? LEGACY_WARNING : null,
        taskId,
      );
      return;
    }
    if (resolved.candidates.length === 0) {
      showLogin(
        defaultLoginUrl(registry),
        defaultLoginWorkspace(registry),
        legacy ? LEGACY_WARNING : "この URL の Workspace は保存されていません。Host に接続してください。",
        taskId,
      );
      return;
    }
    showPicker(
      segments[0] ?? "",
      segments[1] ?? "",
      resolved.candidates,
      legacy ? LEGACY_WARNING : "この URL の Workspace は見つかりません。候補から選んでください。",
      taskId,
    );
  } catch (error) {
    fail(error);
  }
}

window.addEventListener("lapis:task-selected", (event) => {
  const detail = (event as CustomEvent<unknown>).detail;
  if (typeof detail === "string" && detail !== "") {
    setTaskFragment(detail);
  }
});

window.addEventListener("lapis:workspace-switched", (event) => {
  const detail = (event as CustomEvent<unknown>).detail;
  if (typeof detail !== "string") {
    return;
  }
  try {
    applySwitchedInfo(JSON.parse(detail) as ConnectionInfo);
  } catch {
    // 不正な通知では URL と保存済み選択を変更しない。
  }
});

window.addEventListener("pagehide", () => {
  stashTaskFragment(currentTaskFragment());
});

window.addEventListener("error", (event) => fail(event.message));
window.addEventListener("unhandledrejection", (event) => fail(event.reason));
void boot();
