//! 起動時に staging 済みの更新を適用し、本体を起動する小さな launcher。
//!
//! 行うことは 2 つだけである。
//!
//! 1. `<install root>/staging/<version>/` に検証可能な package があれば、layout の
//!    規則どおり `versions/<version>/` へ展開し pointer を進める。
//! 2. `current.json` の version にある `lapis.exe` を起動する。
//!
//! 更新の失敗・staging の不在・manifest の不整合のいずれでも、更新は行わずに現行版を
//! 起動する。manifest の取得や解釈、download はアプリ側の責務であり、launcher は staging
//! と pointer の契約だけを使う（`docs/specs/release.md`）。実装は `lapis_launcher` が持つ。

fn main() -> std::process::ExitCode {
    let arguments: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    lapis_launcher::exit_code(lapis_launcher::run(arguments))
}
