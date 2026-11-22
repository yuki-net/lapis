# Lapis vendored gpui-pre-web

このディレクトリは crates.io の `gpui-pre-web 0.3.9`（Zed `gpui_web` の
スナップショット `zed@93d7456`）を基準とするローカル版です。

`--cfg=web_sys_unstable_apis` を有効にすると、web-sys は `getImageData` を
IDL に忠実な `i32` 版として生成し、安定版の `f64` 版はコンパイル対象から外れます。
本家 `gpui-pre-web` は `f64` 版を前提にしているため、そのままでは wasm ビルドが通りません。

WebTransport の web-sys バインディングは同じ cfg を要求するため、本事業では
この cfg を有効にし、差分をこのローカル版で吸収しています。

## 差分

| 対象 | 内容 | 削除条件 |
| --- | --- | --- |
| `src/canvas_text.rs` | `get_image_data` の引数を `i32` に追随（幅・高さは `u32` から変換） | 本家が `web_sys_unstable_apis` 有効時にビルドできるか、`f64`/`i32` の差を吸収したら削除できる |

## 更新時の確認

- 基準版の更新は `Cargo.toml` の `[patch.crates-io]` と本ディレクトリを同時に置き換える
- `cargo check -p web --target wasm32-unknown-unknown` と
  `cargo check -p desktop-ui` の双方で確認する
