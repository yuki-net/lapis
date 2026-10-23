# Welcome to GPUI!

## Lapisでの保守境界

GPUI更新時に残す変更を判断する開発者向けの記録。依存は公式crates.ioの0.2.2
（Zed `69e2130295c2649963eb639fc70b4f2ee8ea1624`、`crates/gpui`）を元にしている。
公式crateのchecksumは`979b45cfa6ec723b6f42330915a1b3769b930d02b2d505f9697f8ca602bee707`。
以下のpatchは改行を正規化した公式ソースとの差分であり、同じファイルに複数の責務が含まれる場合も分けて記録する。

| patch（リポジトリルートの`patches/`） | 目的・対象 |
| --- | --- |
| `gpui-0.2.2-external-inspector.patch` | 全要素ID・親子・Boundsの収集、任意選択、Tree Hover、独立表示と全幅維持、Entity通知、Overlay描画callback。`element.rs`、`inspector.rs`、`window.rs`、`view.rs`と`app.rs`の一部。 |
| `gpui-0.2.2-drag.patch` | 開始したマウスボタンの解放だけでdragを終了する。`app.rs`、`elements/div.rs`、`window.rs`の一部。Inspectorとは独立。 |
| `gpui-0.2.2-windows.patch` | Native移動・リサイズ中の描画と処理継続、non-client領域の既定処理、DirectX Debug Layerのopt-in。`platform/windows/events.rs`、`directx_devices.rs`。 |

公式の`set_inspector_renderer`、`register_inspector_element`、Pick・深さ変更、Div状態・Style・生成元を利用する。
公式`toggle_inspector`は右側30remを予約し、全ツリー列挙・任意選択・Tree Hoverの公開APIを持たないため、外部検査APIを残す。
LapisのUI状態、折り畳み、スクロール、外部Windowのhandleと購読、Box Modelの色・形状はGPUIに置かない。
外部Window更新は`Context::notify`とLapisの`App::observe`で接続する。
Overlay callbackを登録しなければ公式の単色Pick描画を使用する。

Inspector無効時は要素ID・ツリーを収集しない。起動中は描画に合わせてsnapshotを比較し、変化した場合だけコピー・revision更新する。
選択・Hover状態の再取得もEntityへ通知する。定期収集や全Windowの定期再描画は追加しない。
cached viewの子も検査するため、Inspector使用中はview cacheを使わない。終了するとFrame内の検査用バッファも解放する。

Inspector用patchの削除条件は、全幅の独立Window、全ツリー、任意選択、Hover、同等の可視性を公式公開APIで維持し、回帰テストを通せること。
Windows・drag patchは別に操作回帰を検証して判断し、Inspector移行と一緒に削除しない。

リポジトリルートで各patchに対し次を実行すると、保存差分とvendorの整合を確認できる。

```powershell
git apply --reverse --check --directory=vendor/gpui patches/gpui-0.2.2-external-inspector.patch
git apply --reverse --check --directory=vendor/gpui patches/gpui-0.2.2-drag.patch
git apply --reverse --check --directory=vendor/gpui patches/gpui-0.2.2-windows.patch
```

独立Inspectorの描画・開閉はDesktop UIの`devtools`テストと、Debugの`scripts/test_window_host.ps1`内のInspector smokeで検証する。
Native smokeのフレーム計測はGPU presentを含まない短時間の構築時間であり、実操作や継続負荷の保証ではない。

GPUI is a hybrid immediate and retained mode, GPU accelerated, UI framework
for Rust, designed to support a wide variety of applications.

## Getting Started

GPUI is still in active development as we work on the Zed code editor, and is still pre-1.0. There will often be breaking changes between versions. You'll also need to use the latest version of stable Rust and be on macOS or Linux. Add the following to your `Cargo.toml`:

```toml
gpui = { version = "*" }
```

 - [Ownership and data flow](src/_ownership_and_data_flow.rs)

Everything in GPUI starts with an `Application`. You can create one with `Application::new()`, and kick off your application by passing a callback to `Application::run()`. Inside this callback, you can create a new window with `App::open_window()`, and register your first root view. See [gpui.rs](https://www.gpui.rs/) for a complete example.

### Dependencies

GPUI has various system dependencies that it needs in order to work.

#### macOS

On macOS, GPUI uses Metal for rendering. In order to use Metal, you need to do the following:

- Install [Xcode](https://apps.apple.com/us/app/xcode/id497799835?mt=12) from the macOS App Store, or from the [Apple Developer](https://developer.apple.com/download/all/) website. Note this requires a developer account.

> Ensure you launch Xcode after installing, and install the macOS components, which is the default option.

- Install [Xcode command line tools](https://developer.apple.com/xcode/resources/)

  ```sh
  xcode-select --install
  ```

- Ensure that the Xcode command line tools are using your newly installed copy of Xcode:

  ```sh
  sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer
  ```

## The Big Picture

GPUI offers three different [registers](<https://en.wikipedia.org/wiki/Register_(sociolinguistics)>) depending on your needs:

- State management and communication with `Entity`'s. Whenever you need to store application state that communicates between different parts of your application, you'll want to use GPUI's entities. Entities are owned by GPUI and are only accessible through an owned smart pointer similar to an `Rc`. See the `app::context` module for more information.

- High level, declarative UI with views. All UI in GPUI starts with a view. A view is simply an `Entity` that can be rendered, by implementing the `Render` trait. At the start of each frame, GPUI will call this render method on the root view of a given window. Views build a tree of `elements`, lay them out and style them with a tailwind-style API, and then give them to GPUI to turn into pixels. See the `div` element for an all purpose swiss-army knife of rendering.

- Low level, imperative UI with Elements. Elements are the building blocks of UI in GPUI, and they provide a nice wrapper around an imperative API that provides as much flexibility and control as you need. Elements have total control over how they and their child elements are rendered and can be used for making efficient views into large lists, implement custom layouting for a code editor, and anything else you can think of. See the `element` module for more information.

Each of these registers has one or more corresponding contexts that can be accessed from all GPUI services. This context is your main interface to GPUI, and is used extensively throughout the framework.

## Other Resources

In addition to the systems above, GPUI provides a range of smaller services that are useful for building complex applications:

- Actions are user-defined structs that are used for converting keystrokes into logical operations in your UI. Use this for implementing keyboard shortcuts, such as cmd-q. See the `action` module for more information.

- Platform services, such as `quit the app` or `open a URL` are available as methods on the `app::App`.

- An async executor that is integrated with the platform's event loop. See the `executor` module for more information.,

- The `[gpui::test]` macro provides a convenient way to write tests for your GPUI applications. Tests also have their own kind of context, a `TestAppContext` which provides ways of simulating common platform input. See `app::test_context` and `test` modules for more details.

Currently, the best way to learn about these APIs is to read the Zed source code, ask us about it at a fireside hack, or drop a question in the [Zed Discord](https://zed.dev/community-links). We're working on improving the documentation, creating more examples, and will be publishing more guides to GPUI on our [blog](https://zed.dev/blog).
