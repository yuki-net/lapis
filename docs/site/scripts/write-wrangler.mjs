#!/usr/bin/env node
// GitHub Actions の secrets / vars から wrangler.json を生成する。
// 値そのものは出力せず、欠落している変数名だけを報告する。

import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const OUTPUT = resolve(ROOT, 'wrangler.json');

// 必須: 欠落時はデプロイを中止する。任意: 未設定なら既定値を使う。
const REQUIRED = ['CLOUDFLARE_ACCOUNT_ID', 'CLOUDFLARE_API_TOKEN', 'CLOUDFLARE_PAGES_PROJECT'];

function read(name) {
  const value = process.env[name];
  return typeof value === 'string' ? value.trim() : '';
}

function fail(lines) {
  console.error('wrangler.json を生成できませんでした。');
  for (const line of lines) console.error(`- ${line}`);
  console.error(
    'GitHub の Environment "cloudflare-pages" に secrets（CLOUDFLARE_ACCOUNT_ID / CLOUDFLARE_API_TOKEN）と ' +
      'vars（CLOUDFLARE_PAGES_PROJECT など）を設定してください。値はログに表示しません。',
  );
  process.exit(1);
}

const missing = REQUIRED.filter((name) => read(name) === '');
if (missing.length > 0) {
  fail([`未設定の変数: ${missing.join(', ')}`]);
}

const project = read('CLOUDFLARE_PAGES_PROJECT');
if (!/^[a-z0-9](?:[a-z0-9-]{0,57}[a-z0-9])?$/.test(project)) {
  fail(['CLOUDFLARE_PAGES_PROJECT は英小文字・数字・ハイフン（1〜59文字）で指定してください。']);
}

// Pages の設定は account_id を受け付けない。アカウントとトークンは wrangler が
// 環境変数から読むため、ここでは形式だけ確認して値を設定へ書かない。
if (!/^[0-9a-f]{32}$/.test(read('CLOUDFLARE_ACCOUNT_ID'))) {
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
