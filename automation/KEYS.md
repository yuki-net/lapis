# 認証と秘密（owner 名義）

## 採用

- コミット・PR・merge は **オーナーアカウント名義**で行う
- worker と coordinator は、オーナーの `gh` / git 認証を使う
- verifier / skeptic / architect / minimalist は、オーナーの認証で読む
- AI API キーはロールごとに分ける（可能な範囲で）

## 不採用

- GitHub App、installation token、private key
- `orcaworker` ユーザー、サービスアカウント、localhost ブローカー

同一アカウント運用なので、認証情報の分離では「worker は merge しない」「reviewer は
書き込まない」を守れない。**これらはプロンプトと手順の制約**であり、GitHub の権限ではない。

## 秘密の置き場

```text
%USERPROFILE%\.lapis\
  ai\worker.env       # worker の AI API キー
  ai\reviewer.env     # reviewer の AI API キー

%LOCALAPPDATA%\lapis\locks\   # dispatch のロック（秘密ではない）
```

- `gh` の認証はオーナーの keyring をそのまま使う
- token をプロンプト・ログ・成果物・シェル履歴に出さない
- 秘密は repo に置かない
- `scripts/ci/test_no_tracked_secrets.py` が追跡ファイルを検査する
- `.gitignore` は事故防止のバックストップ。保管場所ではない

## ローテーション

1. AI API キーを新しくする
2. `%USERPROFILE%\.lapis\ai\` の `.env` を差し替える
3. 次の dispatch / review で新しいキーになることを確認

`gh` の認証はオーナーの管理に従う。worker / coordinator から再ログインしない。

## 禁止

- repo 内に `.env` / AI キー / token を置く
- token をプロンプトやログに書く
- 保護パス / `.github/workflows` を AI が変更する
- verifier PASS なしで coordinator が merge する
- reviewer が GitHub にコメント・review・label を付ける

## 注意

同一アカウント運用のため、上記の禁止は GitHub の権限では強制されない。
owner の bypass 権限は存在する。手順とプロンプトで守る。
