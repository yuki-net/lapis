# 拡張 package の作成

拡張の開発者が `.lapis` を作成するためのコマンド。ソースは Lapis の workspace の外に置ける。
`extension.json` は入力用であり、署名済み package の `manifest.json` とは別である。

```json
{
  "id": "dev.example.hello",
  "version": "0.1.0",
  "requires": [{"contract": "lapis.extension", "minimum": "1.0.0", "maximum": "1.0.0"}],
  "files": ["extension.wasm"]
}
```

```sh
python -m scripts.extensions.package <source-directory> <output.lapis> --development
python -m scripts.extensions.package <source-directory> <output.lapis> --seed-file <private-seed-file> --key-id <publisher-id>
```

開発 package は未署名で、明示的な開発モードの Host にのみ導入できる。署名には 32 byte の
Ed25519 seed を file から読む。秘密鍵を repository や package に含めない。署名済み package の
発行者公開鍵は Host 管理者が信頼鍵一覧へ明示的に追加する。読み込み時の厳密な manifest・
署名・ハッシュ・互換性の検証は Host が行う。

同梱拡張は `extensions/<name>/extension.json` の存在で自動発見し、repository check で
package 作成を検証する。まだ package 化していない既存のディレクトリは対象にしない。

リリース時は `python -m scripts.extensions.bundle extensions <同梱先>` で発見した拡張を
リリース署名鍵で署名する。署名鍵はリリース処理と同じ `LAPIS_RELEASE_SIGNING_KEY` の
ファイルから読み、配布物には含めない。Host は通常の外部拡張と同じ検証を通して初回導入する。

`extension.json` に `"capabilities": [{"id": "process.pty", "version": "1.0.0"}]`
を追加すると schema 2 の署名対象として記録する。能力は申請だけでは使えず、Host の
承認が必要。現在、承認操作と実行時の能力付与は未接続。
