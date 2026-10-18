import { spawnSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const root = fileURLToPath(new URL("../../../", import.meta.url));
const web = fileURLToPath(new URL("../", import.meta.url));
const bindingsVersion = "0.2.129";

function run(command, args) {
  const result = spawnSync(command, args, { cwd: root, stdio: "inherit" });
  if (result.error || result.status !== 0) {
    throw new Error(`${command} failed: ${result.error?.message ?? result.status}`);
  }
}

try {
  if (process.argv.includes("--setup")) {
    run("rustup", ["target", "add", "wasm32-unknown-unknown"]);
    run("cargo", ["install", "wasm-bindgen-cli", "--version", bindingsVersion, "--locked"]);
  } else {
    const version = spawnSync("wasm-bindgen", ["--version"], { encoding: "utf8" });
    if (version.status !== 0 || version.stdout.trim() !== `wasm-bindgen ${bindingsVersion}`) {
      throw new Error("Run npm run setup in apps/web first (Rust and wasm-bindgen are required).");
    }
    run("cargo", ["build", "--locked", "-p", "lapis-web", "--target", "wasm32-unknown-unknown", "--profile", "web"]);
    const metadata = spawnSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], { cwd: root, encoding: "utf8" });
    if (metadata.status !== 0) throw new Error("Cargo target directory could not be determined.");
    const targetDirectory = JSON.parse(metadata.stdout).target_directory;
    const output = path.join(web, "src/wasm");
    mkdirSync(output, { recursive: true });
    run("wasm-bindgen", [
      path.join(targetDirectory, "wasm32-unknown-unknown/web/lapis_web.wasm"),
      "--target", "web", "--out-dir", output, "--out-name", "lapis_web",
    ]);
  }
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
