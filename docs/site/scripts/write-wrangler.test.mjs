// write-wrangler.mjs の検査。値の欠落を検出し、値をログへ出さないことを確認する。
// 生成器は import.meta.url から出力先を決めるため、同じ構成で一時ディレクトリへ
// コピーして実行し、実リポジトリの wrangler.json に触れない。
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const SOURCE = resolve(dirname(fileURLToPath(import.meta.url)), 'write-wrangler.mjs');

const SELECTED = {
  CLOUDFLARE_ACCOUNT_ID: '0123456789abcdef0123456789abcdef',
  CLOUDFLARE_API_TOKEN: 'api-token-secret-value',
  CLOUDFLARE_PAGES_PROJECT: 'lapis-docs',
};

function sandbox() {
  const root = mkdtempSync(join(tmpdir(), 'lapis-wrangler-'));
  const scripts = join(root, 'scripts');
  mkdirSync(scripts);
  copyFileSync(SOURCE, join(scripts, 'write-wrangler.mjs'));
  return { root, config: join(root, 'wrangler.json'), script: join(scripts, 'write-wrangler.mjs') };
}

function run(script, env) {
  return spawnSync(process.execPath, [script], {
    cwd: dirname(dirname(script)),
    env: { ...process.env, ...SELECTED, ...env },
    encoding: 'utf8',
  });
}

test('値が欠けていると変数名つきで失敗する', () => {
  const box = sandbox();
  try {
    const result = run(box.script, { CLOUDFLARE_API_TOKEN: '' });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /CLOUDFLARE_API_TOKEN/);
    assert.doesNotMatch(result.stdout + result.stderr, /0123456789abcdef0123456789abcdef/);
    assert.equal(existsSync(box.config), false);
  } finally {
    rmSync(box.root, { recursive: true, force: true });
  }
});

test('前後に空白を含む値は失敗する', () => {
  const box = sandbox();
  try {
    const result = run(box.script, { CLOUDFLARE_ACCOUNT_ID: `${SELECTED.CLOUDFLARE_ACCOUNT_ID}\n` });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /CLOUDFLARE_ACCOUNT_ID/);
    assert.equal(existsSync(box.config), false);
  } finally {
    rmSync(box.root, { recursive: true, force: true });
  }
});

test('アカウントIDの形式が不正なら失敗する', () => {
  const box = sandbox();
  try {
    const result = run(box.script, { CLOUDFLARE_ACCOUNT_ID: 'not-an-account' });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /CLOUDFLARE_ACCOUNT_ID/);
    assert.equal(existsSync(box.config), false);
  } finally {
    rmSync(box.root, { recursive: true, force: true });
  }
});

test('プロジェクト名が不正なら失敗する', () => {
  const box = sandbox();
  try {
    const result = run(box.script, { CLOUDFLARE_PAGES_PROJECT: 'Lapis Docs' });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /CLOUDFLARE_PAGES_PROJECT/);
    assert.equal(existsSync(box.config), false);
  } finally {
    rmSync(box.root, { recursive: true, force: true });
  }
});

test('設定を生成し、値はログに出さない', () => {
  const box = sandbox();
  try {
    const result = run(box.script, {});
    assert.equal(result.status, 0, result.stderr);
    assert.doesNotMatch(result.stdout + result.stderr, /0123456789abcdef0123456789abcdef|api-token-secret-value/);
    const config = JSON.parse(readFileSync(box.config, 'utf8'));
    assert.equal(config.name, 'lapis-docs');
    assert.equal(config.pages_build_output_dir, './dist');
    // Pages の設定は account_id を受け付けない（値は環境変数から渡す）。
    assert.equal('account_id' in config, false);
  } finally {
    rmSync(box.root, { recursive: true, force: true });
  }
});
