import fs from "node:fs/promises"
import process from "node:process"

import * as core from "@actions/core"
import * as github from "@actions/github"
import { createOpencode } from "@opencode-ai/sdk"

import { assertEnv, buildResponseText, withTimeout } from "../../_shared/opencode.mjs"

const COMMAND_RE = /(^|\s)\/(?:oc|opencode)\b/u
const MAX_PROMPT_CONTEXT_CHARS = 40_000
const MAX_ISSUE_BODY_CHARS = 12_000
const MAX_COMMENT_BODY_CHARS = 4_000
const MAX_PR_BODY_CHARS = 12_000
const MAX_DIFF_HUNK_CHARS = 12_000

function shouldRun(body) {
  const text = String(body || "")
  return COMMAND_RE.test(text)
}

function extractPrompt(body) {
  const text = String(body || "").trim()
  if (!text) return ""
  if (!COMMAND_RE.test(text)) return ""

  let removed = false
  const prompt = text
    .replace(COMMAND_RE, (match, prefix = "") => {
      removed = true
      return prefix
    })
    .trim()
  if (!removed) return ""
  return prompt
}

function truncateForGitHub(body, max = 60000) {
  const text = String(body || "")
  if (text.length <= max) return text
  return `${text.slice(0, max - 20)}\n\n[truncated]\n`
}

function truncateForPrompt(text, max) {
  const value = String(text || "")
  if (!Number.isFinite(max) || max <= 0) return ""
  if (value.length <= max) return value
  const suffix = "\n\n[truncated]\n"
  const head = Math.max(0, max - suffix.length)
  return `${value.slice(0, head)}${suffix}`
}

async function safeReadJson(path) {
  const raw = await fs.readFile(path, "utf-8")
  return JSON.parse(raw)
}

async function formatIssueThread(octokit, { owner, repo, issueNumber }) {
  const [issue, comments] = await Promise.all([
    octokit.rest.issues.get({ owner, repo, issue_number: issueNumber }),
    octokit.rest.issues.listComments({
      owner,
      repo,
      issue_number: issueNumber,
      sort: "created",
      direction: "desc",
      per_page: 30,
    }),
  ])

  const parts = []
  parts.push(`# ${truncateForPrompt(issue.data.title, 512)}\n`)
  if (issue.data.body) {
    parts.push(`## Body\n${truncateForPrompt(issue.data.body, MAX_ISSUE_BODY_CHARS)}\n`)
  }
  if (comments.data.length > 0) {
    parts.push("## Comments")
    const commentsForPrompt = [...comments.data].reverse()
    for (const c of commentsForPrompt) {
      const who = c.user?.login || "unknown"
      const when = c.created_at || ""
      const body = truncateForPrompt(c.body || "", MAX_COMMENT_BODY_CHARS)
      parts.push(`- ${who} ${when}\n${body}\n`)
    }
    parts.push("")
  }

  return truncateForPrompt(parts.join("\n"), MAX_PROMPT_CONTEXT_CHARS)
}

async function formatPullRequest(octokit, { owner, repo, pullNumber }) {
  const pr = await octokit.rest.pulls.get({ owner, repo, pull_number: pullNumber })
  const parts = []
  parts.push(`# ${truncateForPrompt(pr.data.title, 512)}\n`)
  if (pr.data.body) {
    parts.push(`## Body\n${truncateForPrompt(pr.data.body, MAX_PR_BODY_CHARS)}\n`)
  }
  return truncateForPrompt(parts.join("\n"), MAX_PROMPT_CONTEXT_CHARS)
}

async function run() {
  const githubToken = assertEnv("GITHUB_TOKEN")
  const eventName = assertEnv("GITHUB_EVENT_NAME")
  const eventPath = assertEnv("GITHUB_EVENT_PATH")

  const payload = await safeReadJson(eventPath)

  const octokit = github.getOctokit(githubToken)
  const owner = payload?.repository?.owner?.login || process.env.GITHUB_REPOSITORY_OWNER
  const repo = payload?.repository?.name || (process.env.GITHUB_REPOSITORY || "").split("/")[1]

  if (!owner || !repo) {
    throw new Error("unable to determine repo owner/name from payload/env")
  }

  if (eventName === "issue_comment") {
    const issueNumber = payload?.issue?.number
    const comment = payload?.comment
    const commentBody = comment?.body || ""
    const commenter = comment?.user?.login || ""
    const commenterType = comment?.user?.type || ""

    if (!issueNumber || !comment) return
    if (commenterType === "Bot" || commenter.endsWith("[bot]")) return
    if (!shouldRun(commentBody)) return

    const prompt = extractPrompt(commentBody)
    if (!prompt) return

    console.log("🚀 Starting opencode server...")
    const opencode = await createOpencode({ port: 0 })
    console.log("✅ Opencode server ready")

    const contextText = await formatIssueThread(octokit, { owner, repo, issueNumber })
    const created = await withTimeout(
      (signal) =>
        opencode.client.session.create(
          {
            body: { title: `GitHub ${owner}/${repo}#${issueNumber}` },
          },
          { signal },
        ),
      "github-action session.create(issue_comment)",
    )
    if (created.error) {
      throw new Error(created.error.message || "failed to create session")
    }

    const sessionId = created.data.id
    let url = null
    try {
      const shared = await withTimeout(
        (signal) => opencode.client.session.share({ path: { id: sessionId } }, { signal }),
        "github-action session.share(issue_comment)",
      )
      url = shared?.data?.share?.url || null
    } catch (err) {
      core.warning(`session share failed: ${err?.message || String(err)}`)
    }

    const result = await withTimeout(
      (signal) =>
        opencode.client.session.prompt(
          {
            path: { id: sessionId },
            body: {
              parts: [
                {
                  type: "text",
                  text: `You are responding to a GitHub issue comment.\n\n${contextText}\n\n## Request\n${prompt}\n`,
                },
              ],
            },
          },
          { signal },
        ),
      "github-action session.prompt(issue_comment)",
    )

    if (result.error) {
      throw new Error(result.error.message || "opencode prompt failed")
    }
    if (!result.data) {
      throw new Error("opencode prompt failed")
    }

    const responseText = buildResponseText(result.data)

    const body = truncateForGitHub([url ? `OpenCode session: ${url}` : null, responseText].filter(Boolean).join("\n\n"))
    await octokit.rest.issues.createComment({
      owner,
      repo,
      issue_number: issueNumber,
      body,
    })

    return
  }

  if (eventName === "pull_request_review_comment") {
    const pullNumber = payload?.pull_request?.number
    const comment = payload?.comment
    const commentBody = comment?.body || ""
    const commenter = comment?.user?.login || ""
    const commenterType = comment?.user?.type || ""

    if (!pullNumber || !comment) return
    if (commenterType === "Bot" || commenter.endsWith("[bot]")) return
    if (!shouldRun(commentBody)) return

    const prompt = extractPrompt(commentBody)
    if (!prompt) return

    console.log("🚀 Starting opencode server...")
    const opencode = await createOpencode({ port: 0 })
    console.log("✅ Opencode server ready")

    const prText = await formatPullRequest(octokit, { owner, repo, pullNumber })
    const codeContext = [
      "## Code context",
      `path: ${comment?.path || ""}`,
      `line: ${comment?.line ?? ""}`,
      "",
      "```diff",
      truncateForPrompt(String(comment?.diff_hunk || "").trim(), MAX_DIFF_HUNK_CHARS),
      "```",
      "",
    ].join("\n")

    const created = await withTimeout(
      (signal) =>
        opencode.client.session.create(
          {
            body: { title: `GitHub ${owner}/${repo}#${pullNumber}` },
          },
          { signal },
        ),
      "github-action session.create(review_comment)",
    )
    if (created.error) {
      throw new Error(created.error.message || "failed to create session")
    }

    const sessionId = created.data.id
    let url = null
    try {
      const shared = await withTimeout(
        (signal) => opencode.client.session.share({ path: { id: sessionId } }, { signal }),
        "github-action session.share(review_comment)",
      )
      url = shared?.data?.share?.url || null
    } catch (err) {
      core.warning(`session share failed: ${err?.message || String(err)}`)
    }

    const result = await withTimeout(
      (signal) =>
        opencode.client.session.prompt(
          {
            path: { id: sessionId },
            body: {
              parts: [
                {
                  type: "text",
                  text: `You are responding to a GitHub PR review comment.\n\n${prText}\n\n${codeContext}\n## Request\n${prompt}\n`,
                },
              ],
            },
          },
          { signal },
        ),
      "github-action session.prompt(review_comment)",
    )

    if (result.error) {
      throw new Error(result.error.message || "opencode prompt failed")
    }
    if (!result.data) {
      throw new Error("opencode prompt failed")
    }

    const responseText = buildResponseText(result.data)

    const body = truncateForGitHub([url ? `OpenCode session: ${url}` : null, responseText].filter(Boolean).join("\n\n"))

    await octokit.rest.pulls.createReplyForReviewComment({
      owner,
      repo,
      pull_number: pullNumber,
      comment_id: comment.id,
      body,
    })

    return
  }

  console.log(`unsupported event: ${eventName}; skipping`)
}

run().catch((err) => {
  core.setFailed(err?.message || String(err))
})
