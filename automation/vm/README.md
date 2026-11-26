# Linux VM と Orca の pair

## 第一候補

- Hyper-V + Ubuntu 24.04 LTS
- 1 VM、2 UNIX ユーザー（`lapis-worker` / `lapis-reviewer`）
- 各ユーザーが `orca serve` を起動し、ホストの Orca が pair する

```text
ホスト Orca
  environment worker-env   → VM :6768
  environment reviewer-env → VM :6769
```

## VM に入れるもの

- git / curl / build-essential / pkg-config
- Python 3.12
- Node 24（Web の検証用）
- Java 21（Mobile の検証用。Android SDK は必要になったら入れる）
- Rust 1.96（rustfmt / clippy）
- wasm-bindgen 0.2.129
- OpenCode（基本）と Junie CLI（fallback）
- Orca の Linux ランタイム

## ユーザー

| ユーザー | 持つもの | 持たないもの |
| --- | --- | --- |
| `lapis-worker` | worker App の鍵、OpenCode、repo / worktree、Cargo / Gradle キャッシュ | reviewer の鍵、AI reviewer キー |
| `lapis-reviewer` | reviewer App の鍵、OpenCode / Junie、dashboard | worker の鍵 |

- worker に sudo を渡さない
- 鍵は各 home に 600
- ホストの `gh` token は VM に置かない

## pairing

VM の各ユーザーで:

```bash
orca serve --bind 0.0.0.0 --port 6768 \
  --pairing-address <host から届くアドレス> \
  --data-root ~/.local/share/orca --project-root ~/lapis
```

- 起動ログの pairing link を、ホストで:

```powershell
orca environment add --name worker-env --pairing-code "orca://pair?code=..."
```

- pair 後は `--environment worker-env` で worktree / agent を VM 内に作れる
- pairing link は起動ごとに変わる。起動スクリプトでログから拾う（M2）
- `--bind 0.0.0.0` で待ち受けるが、Hyper-V の内部スイッチ / NAT でホストからだけ
  届くようにし、`--allow-public` は使わない

## WSL2 を使う場合

- `/etc/wsl.conf` で automount を無効にする（`/mnt/c` を作らない）
- worker に sudo を渡さない
- repo は ext4 側
- Hyper-V より隔離が弱いので、第一候補は Hyper-V
