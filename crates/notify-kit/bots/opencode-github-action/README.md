# opencode-github-action

一个最小可用的 GitHub Actions “评论 bot / 集成示例”，用于把 GitHub Issue/PR 评论桥接到 OpenCode session：

- Issue/PR comment → session（每次运行创建新 session 并回贴分享链接）
- 把用户指令转发到 `session.prompt`
- 把模型回复作为评论（或 review comment reply）发回 GitHub

## 依赖

- GitHub Actions runner（或本地 Node.js 20+）
- `GITHUB_TOKEN`（用来读线程 + 发评论）
- 你的 OpenCode server/模型所需的环境变量（按你自己的 OpenCode 配置来）

## 触发方式

默认只在评论中包含以下标记时才会运行：

- `/oc`
- `/opencode`

示例：

```text
/opencode explain this issue
```

```text
请帮我补上错误处理 /oc
```

## 使用（推荐：workflow 直接运行脚本）

在你的仓库里添加 `.github/workflows/opencode.yml`（示例）：

```yml
name: opencode

on:
  issue_comment:
    types: [created]
  pull_request_review_comment:
    types: [created]

jobs:
  opencode:
    if: |
      contains(github.event.comment.body, '/oc') ||
      contains(github.event.comment.body, '/opencode')
    runs-on: ubuntu-latest
    permissions:
      contents: read
      pull-requests: write
      issues: write
    steps:
      - uses: actions/checkout@v4

      - uses: actions/setup-node@v4
        with:
          node-version: 20

      - name: Run opencode GitHub bot
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          # 按你的 OpenCode 配置注入模型/Provider API key（示例）：
          # OPENAI_API_KEY: ${{ secrets.OPENAI_API_KEY }}
        run: |
          cd bots/opencode-github-action
          npm install
          npm start
```

## 说明

- 这是“示例实现”：为了保持简单，默认每次触发都会创建新 session（不做持久化映射）。
- PR review comment 事件会优先以 “reply” 的形式回复到对应 review comment。
