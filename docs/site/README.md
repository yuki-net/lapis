# Documentation site

Lapis の利用者向けドキュメントサイトです。Astro Starlight を使い、本文は移行しやすい通常の Markdown (`.md`) を標準とします。

## ローカルで実行

Node.js 24（CI とデプロイ）を使用します。ローカルでは、依存する Astro 7.4 が要求する 22.23.4 以上を使ってください（`package.json` の `engines` は既存の `>=22.22.0` を維持しています）。依存は `package-lock.json` で固定しているため、初回と更新時は `npm install`、再現したいときは `npm ci` を使います。

```sh
cd docs/site
npm install   # 初回、または依存を変えたとき
npm run dev   # http://localhost:4321
npm run build # dist に静的ファイルを出力
npm test      # wrangler.json 生成スクリプトの検査
```

IntelliJ / RustRover では `.run/Run Docs Site.run.xml` と `.run/Build Docs Site.run.xml` から同じコマンドを実行できます。どちらも `$PROJECT_DIR$/docs/site/package.json` を使うため、先に `npm install` で `node_modules` を用意してください。

本文は `src/content/docs/`。公開予定の操作・設定・外部から観測できる仕様だけを扱い、内部設計文書をコピーしません。見た目や構成はサイト側（`astro.config.mjs` と `src/styles/custom.css`）で管理し、通常の本文に Starlight 固有のコンポーネントを埋め込まないでください。トップページのリンク一覧は本文のリストで書き、カード状の見た目は CSS 側で当てています。

## Cloudflare Pages へのデプロイ

デプロイは `.github/workflows/deploy-docs.yml` が行います。`develop` への `docs/site/**` の push と手動実行の両方が対象で、`wrangler types` による設定検証の後に Pages へアップロードします。`.github/workflows/docs.yml` はビルドとテストだけで、アップロードは行いません。

### リポジトリに置かない値

Wrangler の設定はリポジトリにコミットしません。ビルド時に `docs/site/scripts/write-wrangler.mjs` が環境変数から `wrangler.json` を生成します（`.gitignore` 済み）。値そのものはログに出さず、欠落時は変数名だけを表示して失敗します。

| 変数 | 種別 | 必須 | 用途 |
| --- | --- | --- | --- |
| `CLOUDFLARE_ACCOUNT_ID` | secret | 必須 | 対象アカウント（32桁の16進数） |
| `CLOUDFLARE_API_TOKEN` | secret | 必須 | Pages の編集権限を持つ API トークン |
| `CLOUDFLARE_PAGES_PROJECT` | vars | 必須 | Pages プロジェクト名（英小文字・数字・ハイフン） |
| `CLOUDFLARE_PAGES_BRANCH` | vars | 任意 | デプロイ先ブランチ名。未設定なら push 元のブランチ名 |

これらは GitHub の Environment `cloudflare-pages` にだけ登録します。Environment が無い、または値が空の場合は設定生成の段階で停止するため、原因がそのままログに残ります。初回はリポジトリの Settings → Environments で `cloudflare-pages` を作成してから登録してください。

### 生成される設定

```json
{
  "name": "<CLOUDFLARE_PAGES_PROJECT>",
  "pages_build_output_dir": "./dist",
  "compatibility_date": "2026-11-21"
}
```

Pages の設定は `account_id` を受け付けないため、アカウントは `CLOUDFLARE_ACCOUNT_ID` から wrangler が読みます。出力ディレクトリは `dist` です。

### 手元で確認する

secrets が無い環境ではアップロードまで検証できません。手元では設定生成とビルド、wrangler の設定読み込みまでを次で確認します。

```sh
cd docs/site
CLOUDFLARE_ACCOUNT_ID=<id> CLOUDFLARE_API_TOKEN=<token> \
  CLOUDFLARE_PAGES_PROJECT=lapis-docs npm run wrangler:config
npm run build
npx wrangler types --include-runtime=false <一時ファイルのパス>.d.ts
```

`wrangler pages deploy` は `--dry-run` を持たず、認証情報が無ければすぐに API 呼び出しで失敗します。そのため手元で確認できるのは `wrangler.json` を wrangler が読み込めることまでです。実デプロイは Environment の secrets を設定した後に `Deploy Docs` を手動実行して確認してください。
