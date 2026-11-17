# 外部 workspace の拡張例

Lapis の内部 crate に依存せず、公開 v1 WIT と `wit-bindgen` で作る最小 component。
このディレクトリは独立した Cargo workspace で、別 repository へ移してもビルドできる。

```sh
rustup target add wasm32-wasip2
cargo build --locked --manifest-path examples/external-extension/Cargo.toml --target wasm32-wasip2 --target-dir target/external-extension
```

生成した `example_lapis_extension.wasm` をソースの `extension.wasm` に配置し、
`python -m scripts.extensions.package examples/external-extension output.lapis --development` で
開発用 package を作る。Host の `--development-extensions` は loopback と state directory が必要。
`echo` 操作は公開 dispatch v1 で実行できる。リモート操作には Host 全体の `extensions.execute` 権限が必要。
PTY を使う場合は別の能力契約の宣言と承認が必要。
