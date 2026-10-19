# Backend

- 配置と依存方向は `docs/specs/architecture.md`、機械検査は `scripts/check_architecture.py` を参照する。
- 正規状態と実行資源の寿命は Runtime が所有する。Panel・Tab・focus を持ち込まない。
- 外部 CLI・FS・PTY の実装は infrastructure、操作判断は application、認証・wire は transport。
- 接続切断でプロセスを終了しない。終了は明示操作または Host の終了で行う。
- Client 型や GPUI に依存せず、古い非同期結果を revision・世代で拒否する。
- 例外ファイルを増やして違反を回避しない。
