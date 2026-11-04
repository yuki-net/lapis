import "./style.css";
import init, { is_ready, start } from "./generated/wasm/lapis_web.js";

const root = document.querySelector<HTMLElement>("#app");
if (root === null) {
  throw new Error("Lapis Web root was not found");
}

root.innerHTML = `<section class="loading" role="status"><strong>Lapis Web</strong><span>Desktop UI を読み込んでいます…</span></section>`;

const badge = document.createElement("aside");
badge.className = "web-badge";
badge.textContent = "Web · メモリー上のWorkspace";
document.body.append(badge);

function fail(error: unknown) {
  root!.hidden = false;
  root!.classList.add("error");
  root!.textContent = `起動できません: ${String(error)}。?backend=webgl を付けるか、WebGPU/WebGL2 が有効なブラウザーで開いてください。`;
  document.body.dataset.lapis = "error";
}

async function boot() {
  try {
    await init();
    start(new URLSearchParams(location.search).get("backend") ?? "auto");
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
