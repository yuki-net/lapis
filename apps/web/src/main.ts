import "./style.css";
import init, { is_ready, start } from "./generated/wasm/lapis_ui.js";

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

function fail(error: unknown) {
  root!.hidden = false;
  root!.classList.add("error");
  root!.textContent = `起動できません: ${String(error)}。?backend=webgl を付けるか、WebGPU/WebGL2 が有効なブラウザーで開いてください。`;
  document.body.dataset.lapis = "error";
}

async function boot() {
  try {
    const params = new URLSearchParams(location.search);
    const url = params.get("url") ?? localStorage.getItem(STORAGE.url) ?? "";
    const workspace =
      params.get("workspace") ?? localStorage.getItem(STORAGE.workspace) ?? "";
    const token = params.get("token") ?? "";
    const clientId = localStorage.getItem(STORAGE.clientId) ?? createClientId();
    localStorage.setItem(STORAGE.clientId, clientId);
    const credentialKey = url !== "" && workspace !== "" ? `${url}|${workspace}` : "";
    const credential = credentialKey === "" ? null : localStorage.getItem(credentialKey);
    if (url !== "" && workspace !== "") {
      localStorage.setItem(STORAGE.url, url);
      localStorage.setItem(STORAGE.workspace, workspace);
    }
    await init();
    const fresh = await start(
      params.get("backend") ?? "auto",
      url === "" ? null : url,
      workspace === "" ? null : workspace,
      token === "" ? null : token,
      credential,
      clientId,
    );
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
  } catch (error) {
    fail(error);
  }
}

window.addEventListener("error", (event) => fail(event.message));
window.addEventListener("unhandledrejection", (event) => fail(event.reason));
void boot();
