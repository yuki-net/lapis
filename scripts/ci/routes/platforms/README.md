# OS ごとの追加 step

`<route>_<os>.py` を置くと、その route の `Route.platform_steps` に載り、
その OS で走るときだけ共通 step の後ろに足される。

```python
# scripts/ci/routes/platforms/host_linux.py
from scripts.ci.model import Step

STEPS = (
    Step("Test Host on Linux", ["cargo", "test", "--locked", "-p", "host"]),
)
```

- ファイルを置くだけで効く。route ファイルの変更は要らない
- 差分が無い OS のファイルは作らない（共通 step だけで走る）
- ファイル名の `<os>` は `model.RUNNERS` のキーと一致させる。違えば
  `ValueError` になる（綴り間違いを黙って無視しない）
- `STEPS` は必須
