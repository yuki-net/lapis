# Desktop AGENTS.md

この規約は `apps/desktop/` 配下に適用し、root の `AGENTS.md` を補足します。

## GPUI の責務

- Shell は window、panel、focus、layout と feature の配置だけを所有する。
- 各 feature は自身の UI state、action、view を所有し、Shell の巨大な state へ追加し続けない。
- editor は入力、selection、caret、scroll、描画など、低遅延が必要な状態を所有する。
- component は視覚的な再利用単位とし、業務状態や外部 I/O を所有しない。

新しい `Entity` やモジュールは、独立した状態所有者、ライフサイクル、入力 action、再描画範囲、検証可能な振る舞いのいずれかを境界として作ります。行数を減らすためだけに `Entity` や helper を増やしません。

## 描画と副作用

- `Render` は所有する state から element を構築する責務に限定する。
- render 中にファイル、Git、LSP、Terminal、Task、永続化を操作しない。
- blocking 処理を UI thread で実行しない。
- 非同期結果は対象 Entity の update と Revision または request sequence を通して反映する。
- GPUI の `Context`、`Entity`、event 型を backend の契約へ漏らさない。
- feature 間の状態を直接変更せず、action または明示的な契約を使う。

`mod.rs` と `lib.rs` は宣言と意図的な公開 API に留めます。意味のない再 export や、単一の処理を順番どおり別ファイルへ散らす分割は避けます。
