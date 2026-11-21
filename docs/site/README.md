# Documentation site

Lapis の利用者向けドキュメントサイトです。Astro Starlight を使い、本文は移行しやすい通常の Markdown (`.md`) を標準とします。

## ローカルで実行

Node.js 24（CI）を使用します。ローカルでは、依存する Astro 7.4 が要求する 22.23.4 以上を使ってください（`package.json` の `engines` は既存の `>=22.22.0` を維持しています）。依存は `package-lock.json` で固定しているため、初回と更新時は `npm install`、再現したいときは `npm ci` を使います。

```sh
cd docs/site
npm install   # 初回、または依存を変えたとき
npm run dev   # http://localhost:4321
npm run build # dist に静的ファイルを出力
```

IntelliJ / RustRover では `.run/Run Docs Site.run.xml` と `.run/Build Docs Site.run.xml` から同じコマンドを実行できます。どちらも `$PROJECT_DIR$/docs/site/package.json` を使うため、先に `npm install` で `node_modules` を用意してください。

本文は `src/content/docs/`。公開予定の操作・設定・外部から観測できる仕様だけを扱い、内部設計文書をコピーしません。見た目や構成はサイト側（`astro.config.mjs` と `src/styles/custom.css`）で管理し、通常の本文に Starlight 固有のコンポーネントを埋め込まないでください。トップページのリンク一覧は本文のリストで書き、カード状の見た目は CSS 側で当てています。
