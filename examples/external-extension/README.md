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
実行時に Host が公開するのは現時点では起動・停止だけであり、PTY や設定への操作はまだできない。
