# Lapis での変更

`portable-pty` 0.9.0 を同梱する。Windows の汎用 PTY では、
`CommandBuilder::set_owned_process_tree(true)` を指定した子だけを一時停止して生成し、
終了時に子孫を回収する Job に所属させてから再開する。Job への登録に失敗した場合は
子を終了して起動を失敗させる。指定しない既存の呼び出しは従来の動作を維持する。

Job は通常の `CreateProcess` による子孫を対象とする。OS の別の経路で生成される
プロセスまで隔離するセキュリティ境界ではない。元のライセンスは `LICENSE.md` を参照。
