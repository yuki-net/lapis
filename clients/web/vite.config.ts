import { spawn } from "node:child_process";
import path from "node:path";
import { defineConfig, type Plugin } from "vite";

/// `desktop-ui` の path 依存が収まるディレクトリ。ここにある `.rs` だけを監視する。
const WATCHED_DIRECTORIES = ["clients/ui", "clients/state", "clients/transport", "contracts", "extensions"];
/// 連続保存を1回のビルドにまとめる待ち時間。
const DEBOUNCE_MS = 250;
/// 失敗時に表示する出力の末尾行数。
const FAILURE_LINES = 20;
/// ビルド出力から保持する末尾の文字数。
const OUTPUT_LIMIT = 8_000;

/// `.rs` の保存で Wasm を作り直す。反映は再読み込みで行う。
function wasmRebuild(): Plugin {
  return {
    name: "lapis-wasm-rebuild",
    apply: "serve",
    configureServer(server) {
      // vite は `clients/web` を root にして起動する。リポジトリ直下はその2つ上。
      const repository = path.resolve(server.config.root, "../..");
      server.watcher.add(WATCHED_DIRECTORIES.map((directory) => path.join(repository, directory)));

      let timer: ReturnType<typeof setTimeout> | undefined;
      let running = false;
      let queued = false;

      const build = (): Promise<void> => {
        const started = Date.now();
        // `shell` で起動すると引数の配列は非推奨になるため、コマンドを1つの文字列にする。
        const child = spawn("npm run wasm", {
          cwd: server.config.root,
          shell: true,
          stdio: ["ignore", "pipe", "pipe"],
        });
        let output = "";
        const forward = (chunk: Buffer) => {
          const text = chunk.toString();
          output = (output + text).slice(-OUTPUT_LIMIT);
          for (const line of text.split(/\r?\n/)) {
            if (line.trim() !== "") {
              server.config.logger.info(line);
            }
          }
        };
        child.stdout.on("data", forward);
        child.stderr.on("data", forward);
        return new Promise<void>((resolve) => {
          child.on("error", (error) => {
            server.config.logger.error(`web: wasm build failed: ${error.message}`);
            resolve();
          });
          child.on("close", (code) => {
            if (code === 0) {
              const seconds = ((Date.now() - started) / 1000).toFixed(1);
              server.config.logger.info(`web: wasm rebuilt in ${seconds}s`);
              server.ws.send({ type: "full-reload" });
            } else {
              // 失敗したら古い Wasm を配り続け、修正後の保存でもう一度試す。
              const lines = output.split(/\r?\n/).filter((line) => line.trim() !== "");
              for (const line of lines.slice(-FAILURE_LINES)) {
                server.config.logger.error(line);
              }
              server.config.logger.error(`web: wasm build failed (exit code ${code})`);
            }
            resolve();
          });
        });
      };

      // ビルドは常に1本。実行中の保存は捨てず、終わってから1回だけ合流させる。
      const rebuild = async (): Promise<void> => {
        if (running) {
          queued = true;
          return;
        }
        running = true;
        try {
          do {
            queued = false;
            await build();
          } while (queued);
        } finally {
          running = false;
        }
      };

      const schedule = (file: string) => {
        if (!file.endsWith(".rs")) {
          return;
        }
        if (timer !== undefined) {
          clearTimeout(timer);
        }
        timer = setTimeout(() => {
          timer = undefined;
          void rebuild();
        }, DEBOUNCE_MS);
      };

      server.watcher.on("change", schedule);
      server.watcher.on("add", schedule);
      server.watcher.on("unlink", schedule);
    },
  };
}

export default defineConfig({
  plugins: [wasmRebuild()],
  server: {
    host: "127.0.0.1",
    port: 4747,
    strictPort: true,
  },
});
