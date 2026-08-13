# Backend AGENTS.md

この規約は `backend/` 配下に適用し、root の `AGENTS.md` を補足します。

## Core

- `core` は正規状態、不変条件、application policy、外部機能に求める契約を所有する。
- GPUI、Android、Browser、OS、プロセス、ファイルシステム、Git CLI、PTY、transport の具体実装へ依存しない。
- port は利用する側に置く。adapter の都合で core のモデルを歪めない。
- application service は複数の責務を調停するときだけ使い、単一 feature の処理や正規状態を重複して持たせない。

## Adapter

- `local` と `persistence` は core または feature の契約を実装する。
- 外部形式、エラー、再試行、権限、resource lifecycle を adapter 内に閉じ込める。
- 一つの adapter は一つの外部 resource または一つの変更理由を中心にする。
- 複数 adapter の選択と組み立ては app の entry point に任せる。

## 分離の判断

状態所有、ライフサイクル、外部 resource、契約、回復方針、独立した test fixture のいずれかが異なるときにモジュールを分けます。同じ mutable state を自由に共有したままファイルだけを分けたり、名前だけの repository、manager、service を増やしたりしません。

公開 API は利用者が必要とする操作と型に限定します。内部モジュール、外部ライブラリの型、保存形式を公開契約にしません。
