# 配布物の手動復旧

更新後に起動できない、`current.json` が壊れた、または新しい版を戻したいときに、package を手で差し戻す手順です。対象は per-user 配置（`%LOCALAPPDATA%\Lapis`）で、管理者権限は不要です。layout と manifest の契約は [配布と更新の土台](specs/distribution.md) を正本とします。

署名鍵は不要です。配布物の `manifest.json` と `manifest.pub` だけで、戻す package が本物かを確認できます。

## 前提

| パス | 中身 |
| --- | --- |
| `install\versions\<version>\` | 展開済みの版 |
| `install\current.json` | 現在の版を指す pointer |
| `data\generation-<N>\` | DB・会話・表示状態 |
| `credentials\` | Host 接続 credential などの再登録が要るもの |

`data\` と `credentials\` は版の切り替えで消しません。

## 手順

1. launcher とアプリを終了する。Local Host が残っている場合は `powershell -NoProfile -File scripts/stop_dev_host.ps1` かタスクトレイから止める。実行中のまま差し替えると、読み込み中のファイルが壊れる。
2. `install\current.json` の中身を控える。壊れていれば削除してよい（次回起動時に選び直す）。
3. 戻したい版の package（`lapis-<version>-<platform>.zip`）と、同じ版の `manifest.json` を入手する。
4. `manifest.json` の `assets` から自分のプラットフォームの `sha256` と `size` を取り、zip の SHA-256 と byte 数が一致することを確認する。一致しなければその zip は使わない。
   ```powershell
   (Get-FileHash -Algorithm SHA256 .\lapis-0.1.0-windows-x86_64.zip).Hash.ToLower()
   (Get-Item .\lapis-0.1.0-windows-x86_64.zip).Length
   ```
5. `install\versions\<version>\` を削除し、zip をそのパスへ展開する。zip の root がそのまま版ディレクトリの中身になる。
6. `current.json` を次の形に書き換える。`generation` は戻す版が使うデータ世代に合わせる。
   ```json
   {"schema": 1, "version": "0.1.0", "generation": 1, "activated_at": "2026-11-12T12:00:00Z"}
   ```
7. launcher を起動し、表示中のバージョンが戻した版になっていることを確認する。

## データ世代を跨いで戻す場合

`breaking: true` の release で世代が上がった後に旧版へ戻すときは、旧世代の `data\generation-<N>\` が残っているかを先に確認します。

- 残っている場合: 手順6で `generation` を旧世代の番号にする。データは旧版の続きから再開する。
- 無い場合: 旧世代のデータは復元できない。新規 install と同じ空の状態になり、Workspace の再登録が必要になる。

`credentials\` は世代の外にあるため、どちらの場合も Host への再ログインや pairing のやり直しは不要です。

## 戻せない場合

- zip の SHA-256 が `manifest.json` と一致しない: 配布物が壊れているか改ざんされている。別の入手経路（GitHub Releases の同じ tag）から取り直す。
- `manifest.json` の署名検証に失敗する: 公開鍵と `key_id` の対応が想定と違う。配布元の manifest と `manifest.pub` を確認し、勝手に検証を飛ばさない。
- 展開後に起動だけ失敗する: `versions\` の版を戻す前に、`data\generation-<N>\` の中身が読めるかを確認する。読めない場合はデータ側の問題で、版を戻しても直らない。
