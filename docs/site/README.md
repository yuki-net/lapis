# Documentation site

Lapis の利用者向けドキュメントサイトです。Astro Starlight を使い、本文は移行しやすい通常の Markdown (`.md`) を標準とします。

## ローカルで実行

Node.js 22.22以上（CIはNode.js 24）を使用します。

```sh
cd docs/site
npm install
npm run dev
npm run build
```

本文は `src/content/docs/`。公開予定の操作・設定・外部から観測できる仕様だけを扱い、内部設計文書をコピーしません。見た目や構成はサイト側で管理し、通常の本文に Starlight 固有のコンポーネントを埋め込まないでください。

このPRでは公開デプロイを設定しません。`npm install` により lockfile が生成されますが、CI への固定は後続の環境で依存解決できるときに対応します。
