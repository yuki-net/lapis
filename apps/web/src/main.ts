import "./style.css";
import init, { is_ready, start } from "./wasm/lapis_web.js";

const root = document.querySelector<HTMLElement>("#app");
if (root === null) throw new Error("Lapis Web root was not found");
root.innerHTML = `<section class="loading" role="status"><strong>Lapis Web</strong><span>Loading the Desktop UI…</span></section>`;

const badge = document.createElement("aside");
badge.className = "prototype-badge";
badge.textContent = "Web prototype · in-memory files · reload resets changes";
document.body.append(badge);

function fail(error: unknown) {
  root!.hidden = false;
  root!.textContent = `Lapis Web could not start: ${String(error)}. Try ?backend=webgl or a browser with WebGPU/WebGL2 enabled.`;
  root!.classList.add("error");
  document.body.dataset.lapis = "error";
}

async function boot() {
  try {
    await init();
    start(new URLSearchParams(location.search).get("backend") ?? "auto");
    const deadline = performance.now() + 20_000;
    while (!is_ready()) {
      if (document.body.dataset.lapis === "error") return;
      if (performance.now() > deadline) throw new Error("Graphics initialization timed out");
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
