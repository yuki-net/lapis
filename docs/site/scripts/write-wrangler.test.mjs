// write-wrangler.mjs の検査。値の欠落を検出し、値をログへ出さないことを確認する。
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync, rmSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const SCRIPT = resolve(ROOT, 'scripts/write-wrangler.mjs');
const CONFIG = resolve(ROOT, 'wrangler.json');

function run(env) {
  return spawnSync(process.execPath, [SCRIPT], {
    cwd: ROOT,
    env: { ...process.env, ...env },
    encoding: 'utf8',
  });
}

const SELECTED = {
  CLOUDFLARE_ACCOUNT_ID: '0123456789abcdef0123456789abcdef',
  CLOUDFLARE_API_TOKEN: 'api-token-secret-value',
  CLOUDFLARE_PAGES_PROJECT: 'lapis-docs',
};

test.beforeEach(() => rmSync(CONFIG, { force: true }));
test.after(() => rmSync(CONFIG, { force: true }));

test('値が欠けていると変数名つきで失敗する', () => {
  const result = run({ ...SELECTED, CLOUDFLARE_API_TOKEN: '' });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /CLOUDFLARE_API_TOKEN/);
  assert.doesNotMatch(result.stdout + result.stderr, /0123456789abcdef0123456789abcdef/);
  assert.equal(existsSync(CONFIG), false);
});

test('アカウントIDの形式が不正なら失敗する', () => {
  const result = run({ ...SELECTED, CLOUDFLARE_ACCOUNT_ID: 'not-an-account' });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /CLOUDFLARE_ACCOUNT_ID/);
  assert.equal(existsSync(CONFIG), false);
});

test('プロジェクト名が不正なら失敗する', () => {
  const result = run({ ...SELECTED, CLOUDFLARE_PAGES_PROJECT: 'Lapis Docs' });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /CLOUDFLARE_PAGES_PROJECT/);
  assert.equal(existsSync(CONFIG), false);
});

test('設定を生成し、値はログに出さない', () => {
  const result = run(SELECTED);
  assert.equal(result.status, 0, result.stderr);
  assert.doesNotMatch(result.stdout + result.stderr, /0123456789abcdef0123456789abcdef|api-token-secret-value/);
  const config = JSON.parse(readFileSync(CONFIG, 'utf8'));
  assert.equal(config.name, 'lapis-docs');
  assert.equal(config.pages_build_output_dir, './dist');
  // Pages の設定は account_id を受け付けない（値は環境変数から渡す）。
  assert.equal('account_id' in config, false);
});
