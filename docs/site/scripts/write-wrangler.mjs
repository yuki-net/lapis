#!/usr/bin/env node
// GitHub Actions の secrets / vars から wrangler.json を生成する。
// 値そのものは出力せず、欠落している変数名だけを報告する。

import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const OUTPUT = resolve(ROOT, 'wrangler.json');

// 必須: 欠落時はデプロイを中止する。
const REQUIRED = ['CLOUDFLARE_ACCOUNT_ID', 'CLOUDFLARE_API_TOKEN', 'CLOUDFLARE_PAGES_PROJECT'];

function fail(lines) {
  console.error('wrangler.json を生成できませんでした。');
  for (const line of lines) console.error(`- ${line}`);
  console.error(
    'GitHub の Environment "cloudflare-pages" に secrets（CLOUDFLARE_ACCOUNT_ID / CLOUDFLARE_API_TOKEN）と ' +
      'vars（CLOUDFLARE_PAGES_PROJECT など）を設定してください。値はログに表示しません。',
  );
  process.exit(1);
}

// 前後の空白は wrangler へ渡る値を変質させるため、黙って trim せず失敗させる。
const missing = [];
const padded = [];
for (const name of REQUIRED) {
  const raw = process.env[name] ?? '';
  if (raw.trim() === '') {
    missing.push(name);
  } else if (raw !== raw.trim()) {
    padded.push(name);
  }
}
if (missing.length > 0) {
  fail([`未設定の変数: ${missing.join(', ')}`]);
}
if (padded.length > 0) {
  fail([`前後に空白を含む変数: ${padded.join(', ')}`]);
}

const project = process.env.CLOUDFLARE_PAGES_PROJECT.trim();
if (!/^[a-z0-9](?:[a-z0-9-]{0,57}[a-z0-9])?$/.test(project)) {
  fail(['CLOUDFLARE_PAGES_PROJECT は英小文字・数字・ハイフン（1〜59文字）で指定してください。']);
}

// Pages の設定は account_id を受け付けない。アカウントとトークンは wrangler が
// 環境変数から読むため、ここでは形式だけ確認して値を設定へ書かない。
const account = process.env.CLOUDFLARE_ACCOUNT_ID.trim();
if (!/^[0-9a-f]{32}$/.test(account)) {
  fail(['CLOUDFLARE_ACCOUNT_ID は32桁の16進数で指定してください。']);
}

const config = {
  $schema: './node_modules/wrangler/config-schema.json',
  name: project,
  pages_build_output_dir: './dist',
  compatibility_date: '2026-11-21',
};

mkdirSync(dirname(OUTPUT), { recursive: true });
writeFileSync(OUTPUT, `${JSON.stringify(config, null, 2)}\n`, 'utf8');
console.log('wrangler.json を生成しました（値は表示しません）。');
