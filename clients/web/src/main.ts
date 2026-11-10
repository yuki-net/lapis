import init, { connectionInfo, is_ready, start } from "./generated/wasm/lapis_ui.js";

const root = document.querySelector<HTMLElement>("#app");
if (root === null) {
  throw new Error("Lapis Web root was not found");
}

root.innerHTML = `<section class="loading" role="status"><strong>Lapis Web</strong><span>Desktop UI を読み込んでいます…</span></section>`;

const STORAGE = {
  url: "lapis.url",
  workspace: "lapis.workspace",
  clientId: "lapis.clientId",
};

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

function fail(error: unknown) {
  root!.hidden = false;
  root!.classList.add("error");
  root!.textContent = `起動できません: ${String(error)}。?backend=webgl を付けるか、WebGPU/WebGL2 が有効なブラウザーで開いてください。`;
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

async function launch(
  backend: string,
  url: string | null,
  workspace: string | null,
  token: string | null,
  idToken: string | null,
) {
  const clientId = localStorage.getItem(STORAGE.clientId) ?? createClientId();
  localStorage.setItem(STORAGE.clientId, clientId);
  // 切り替え可能な credential は Workspace に固定されないため、Host 単位で保存する。
  const credentialKey = url ?? "";
  const credential = credentialKey === "" ? null : localStorage.getItem(credentialKey);
  if (url !== null) {
    localStorage.setItem(STORAGE.url, url);
    localStorage.setItem(STORAGE.workspace, workspace ?? "");
  }

  await init();
  const fresh = await start(backend, url, workspace, token, credential, idToken, clientId);
  if (fresh !== undefined && fresh !== null && credentialKey !== "") {
    localStorage.setItem(credentialKey, fresh);
  }

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
  showConnectionInfo(url);
}

type ConnectionInfo = {
  host_id?: string | null;
  workspace_id: string;
  can_switch: boolean;
  workspaces: Array<{ id: string; name: string }>;
};

function showConnectionInfo(url: string | null) {
  const element = document.querySelector<HTMLElement>("#connection-info");
  if (element === null || url === null) {
    return;
  }
  const raw = connectionInfo();
  if (raw === undefined || raw === null) {
    return;
  }
  let info: ConnectionInfo;
  try {
    info = JSON.parse(raw) as ConnectionInfo;
  } catch {
    return;
  }
  element.replaceChildren();
  const identity = document.createElement("span");
  identity.className = "connection-identity";
  identity.textContent = `Host ${info.host_id ?? "未永続化"} / Workspace ${info.workspace_id}`;
  element.append(identity);

  if (info.can_switch && info.workspaces.length > 0) {
    const label = document.createElement("label");
    label.className = "connection-switch";
    label.append(document.createTextNode("Workspace を切り替え"));
    const select = document.createElement("select");
    for (const workspace of info.workspaces) {
      const option = document.createElement("option");
      option.value = workspace.id;
      option.textContent = `${workspace.name} (${workspace.id})`;
      option.selected = workspace.id === info.workspace_id;
      select.append(option);
    }
    select.disabled = info.workspaces.length < 2;
    select.addEventListener("change", () => {
      const params = new URLSearchParams(location.search);
      params.delete("token");
      params.set("workspace", select.value);
      location.search = params.toString();
    });
    label.append(select);
    element.append(label);
  }
  element.hidden = false;
}

async function connect(url: string, workspace: string, token: string, backend: string) {
  const idToken = readOidcToken(url);
  try {
    await launch(backend, url, workspace, token === "" ? null : token, idToken);
  } catch (error) {
    if (idToken === null) {
      throw error;
    }
    clearOidcToken(url);
    if (token === "" && !hasCredential(url)) {
      throw error;
    }
    await launch(backend, url, workspace, token === "" ? null : token, null);
  }
}

async function launchDemo(backend: string) {
  await launch(backend, null, null, null, null);
}

function defaultBridgeUrl(): string {
  // 同梱起動ではページと bridge が同じホストで動くため、bridge の既定ポートを使う。
  const scheme = location.protocol === "https:" ? "wss:" : "ws:";
  return `${scheme}//${location.hostname || "127.0.0.1"}:4757/remote`;
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
  try {
    return sessionStorage.getItem(key);
  } catch {
    return null;
  }
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
  try {
    sessionStorage.removeItem(key);
  } catch {
    // 保存領域へアクセスできない場合は、次の接続でも未認証として扱われる。
  }
}

function hasCredential(url: string): boolean {
  try {
    return localStorage.getItem(url) !== null;
  } catch {
    return false;
  }
}

function showLogin(defaultUrl: string, defaultWorkspace: string) {
  root!.classList.remove("error");
  root!.hidden = false;
  root!.innerHTML = `
    <form class="login" id="login">
      <h1>Lapis Web</h1>
      <p class="login-lead">Host に接続します。pairing token か OIDC ログインが必要です。</p>
      <label>
        <span>Host URL</span>
        <input name="url" type="url" required autocomplete="url"
          placeholder="ws://127.0.0.1:4757/remote"
          value="${escapeHtml(defaultUrl)}" />
      </label>
      <label>
        <span>Workspace ID <small>（空欄なら Host が選択）</small></span>
        <input name="workspace" type="text" autocomplete="off"
          placeholder="workspace-..."
          value="${escapeHtml(defaultWorkspace)}" />
      </label>
      <label>
        <span>Pairing token <small>（初回のみ）</small></span>
        <input name="token" type="password" autocomplete="off" placeholder="64桁の16進数" />
      </label>
      <button type="submit">接続</button>
      <button type="button" class="secondary" id="oidc-login">OIDC でログイン</button>
      <p class="login-error" id="login-error" hidden></p>
      <a class="login-demo" href="?demo=1">Host なしのデモを開く</a>
    </form>
  `;

  const form = document.querySelector<HTMLFormElement>("#login");
  if (form === null) {
    return;
  }

  const read = () => {
    const data = new FormData(form);
    return {
      url: String(data.get("url") ?? "").trim(),
      workspace: String(data.get("workspace") ?? "").trim(),
      token: String(data.get("token") ?? "").trim(),
    };
  };
  const backend = () => new URLSearchParams(location.search).get("backend") ?? "auto";

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const { url, workspace, token } = read();
    if (url === "") {
      showLoginError("Host URL を入力してください");
      return;
    }
    setLoginBusy(true);
    void connect(url, workspace, token, backend()).catch((error) => {
      setLoginBusy(false);
      showLoginError(error);
    });
  });

  form.querySelector<HTMLButtonElement>("#oidc-login")?.addEventListener("click", () => {
    const { url, workspace, token } = read();
    if (url === "") {
      showLoginError("Host URL を入力してください");
      return;
    }
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
      void connect(url, workspace, token, backend()).catch((error) => {
        setLoginBusy(false);
        showLoginError(error);
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

async function boot() {
  try {
    const params = new URLSearchParams(location.search);
    const backend = params.get("backend") ?? "auto";
    const url = (
      params.get("url") ??
      localStorage.getItem(STORAGE.url) ??
      defaultBridgeUrl()
    ).trim();
    const workspace = (
      params.get("workspace") ??
      localStorage.getItem(STORAGE.workspace) ??
      ""
    ).trim();
    const token = (params.get("token") ?? "").trim();
    const hasIdToken = readOidcToken(url) !== null;

    if (
      url !== "" &&
      (token !== "" || hasIdToken || hasCredential(url))
    ) {
      try {
        await connect(url, workspace, token, backend);
      } catch (error) {
        showLogin(url, workspace);
        showLoginError(error);
      }
      return;
    }
    if (params.get("demo") === "1") {
      await launchDemo(backend);
      return;
    }
    showLogin(url, workspace);
  } catch (error) {
    fail(error);
  }
}

window.addEventListener("error", (event) => fail(event.message));
window.addEventListener("unhandledrejection", (event) => fail(event.reason));
void boot();
