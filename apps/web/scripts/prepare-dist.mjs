import { cp, mkdir, rm } from "node:fs/promises";

/// dist を作り直し、tsc の出力先と静的ファイルを揃える。
const root = new URL("../", import.meta.url);
const source = new URL("src/", root);
const dist = new URL("dist/", root);

await rm(dist, { recursive: true, force: true });
await mkdir(new URL("generated/wasm/", dist), { recursive: true });
await cp(new URL("index.html", root), new URL("index.html", dist));
await cp(new URL("style.css", source), new URL("style.css", dist));
await cp(new URL("generated/wasm/", source), new URL("generated/wasm/", dist), {
  recursive: true,
});
