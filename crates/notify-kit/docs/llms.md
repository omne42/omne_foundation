# llms.txt（给 Cursor / Copilot / Claude 等）

本仓库提供一个 **LLM 友好**的文档打包文件：`llms.txt`。

它会把 `docs/`（按 `docs/SUMMARY.md` 顺序）以及 `bots/*/README.md` 组合成一个大文件，方便你把“仓库说明 + 文档 + 示例”一次性喂给大模型做问答/检索/生成代码。

## 如何生成

在仓库根目录执行：

```bash
./scripts/build-llms-txt.sh
```

生成后的文件路径：

- `./llms.txt`

## 如何使用

一个推荐的提问模板：

```text
Documentation:

{paste llms.txt here}

---

Based on the above documentation, answer the following:

{your question}
```

## 注意事项

- `llms.txt` 会去掉 mdBook Rust 代码块里以 `#` 开头的隐藏行（减少噪音）。
- `llms.txt` 可能包含你在 `docs/` / `bots/` 里写的示例配置；请避免把真实 token / webhook URL 写进文档。

