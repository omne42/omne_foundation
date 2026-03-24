import { App } from "@slack/bolt"
import { createOpencode } from "@opencode-ai/sdk"

import { createBotLimiter, createBotSessionStore } from "../../_shared/bootstrap.mjs"
import { ignoreError } from "../../_shared/log.mjs"
import {
  assertEnv,
  buildResponseText,
  getCompletedToolUpdate,
  runEventSubscriptionLoop,
  withTimeout,
} from "../../_shared/opencode.mjs"

const app = new App({
  token: process.env.SLACK_BOT_TOKEN,
  signingSecret: process.env.SLACK_SIGNING_SECRET,
  socketMode: true,
  appToken: process.env.SLACK_APP_TOKEN,
})

assertEnv("SLACK_BOT_TOKEN")
assertEnv("SLACK_SIGNING_SECRET")
assertEnv("SLACK_APP_TOKEN")

console.log("🚀 Starting opencode server...")
const opencode = await createOpencode({ port: 0 })
console.log("✅ Opencode server ready")

const limiter = createBotLimiter()
const store = await createBotSessionStore()

/**
 * sessionKey = `${channel}-${threadTs}`
 * value = { sessionId, channel, threadTs }
 */
const sessions = store.map
const sessionsById = new Map()
const sessionCreateInflight = new Map()
const messageInflight = new Map()
const pendingCounts = new Map()
const sessionPendingLimitValue = Number.parseInt(
  String(process.env.OPENCODE_BOT_SESSION_MAX_PENDING || "64"),
  10,
)
const sessionPendingLimit =
  Number.isFinite(sessionPendingLimitValue) && sessionPendingLimitValue > 0
    ? sessionPendingLimitValue
    : 64

function getStoredSessionId(value) {
  return typeof value === "string" ? value : typeof value?.sessionId === "string" ? value.sessionId : ""
}

function applyStoreEvictions(evicted) {
  if (!Array.isArray(evicted) || evicted.length === 0) return
  for (const [, value] of evicted) {
    const evictedSessionId = getStoredSessionId(value)
    if (evictedSessionId) {
      sessionsById.delete(evictedSessionId)
    }
  }
}

function setStoredSession(sessionKey, session) {
  const previousSessionId = getStoredSessionId(sessions.get(sessionKey))
  if (previousSessionId && previousSessionId !== session.sessionId) {
    sessionsById.delete(previousSessionId)
  }
  const evicted = store.set(sessionKey, session)
  sessionsById.set(session.sessionId, session)
  applyStoreEvictions(evicted)
}

for (const value of sessions.values()) {
  if (!value || typeof value !== "object") continue
  const sessionId = typeof value.sessionId === "string" ? value.sessionId : ""
  const channel = typeof value.channel === "string" ? value.channel : ""
  const threadTs = typeof value.threadTs === "string" ? value.threadTs : ""
  if (!sessionId || !channel || !threadTs) continue
  sessionsById.set(sessionId, { sessionId, channel, threadTs })
}

async function postThreadMessage(channel, threadTs, text) {
  await ignoreError(
    withTimeout(
      app.client.chat.postMessage({
        channel,
        thread_ts: threadTs,
        text,
      }),
      "slack chat.postMessage",
    ),
    "slack postMessage failed",
  )
}

async function ensureSession(channel, threadTs) {
  const sessionKey = `${channel}-${threadTs}`
  const existing = sessions.get(sessionKey)
  const existingSessionId =
    typeof existing === "string"
      ? existing
      : typeof existing?.sessionId === "string"
        ? existing.sessionId
        : ""

  if (existingSessionId) {
    const session = { sessionId: existingSessionId, channel, threadTs }
    if (
      typeof existing !== "object" ||
      existing === null ||
      existing.sessionId !== existingSessionId ||
      existing.channel !== channel ||
      existing.threadTs !== threadTs
    ) {
      setStoredSession(sessionKey, session)
    } else {
      sessionsById.set(existingSessionId, session)
    }
    return session
  }

  const inflight = sessionCreateInflight.get(sessionKey)
  if (inflight) {
    return inflight
  }

  const creating = (async () => {
    const createResult = await withTimeout(
      (signal) =>
        opencode.client.session.create(
          {
            body: { title: `Slack thread ${threadTs}` },
          },
          { signal },
        ),
      "slack session.create",
    )
    if (createResult.error) {
      throw new Error(createResult.error.message || "failed to create session")
    }

    const session = { sessionId: createResult.data.id, channel, threadTs }
    setStoredSession(sessionKey, session)

    let url = null
    try {
      const shareResult = await withTimeout(
        (signal) => opencode.client.session.share({ path: { id: session.sessionId } }, { signal }),
        "slack session.share",
      )
      url = shareResult?.data?.share?.url || null
    } catch (err) {
      console.error("session share failed:", err?.message || err)
    }
    if (url) {
      await postThreadMessage(channel, threadTs, url)
    }

    return session
  })()

  sessionCreateInflight.set(sessionKey, creating)
  try {
    return await creating
  } finally {
    if (sessionCreateInflight.get(sessionKey) === creating) {
      sessionCreateInflight.delete(sessionKey)
    }
  }
}

function enqueueThreadMessage(channel, threadTs, task) {
  const sessionKey = `${channel}-${threadTs}`
  const pending = pendingCounts.get(sessionKey) || 0
  if (pending >= sessionPendingLimit) {
    const err = new Error(
      `session queue is full (sessionKey=${sessionKey}, maxPending=${sessionPendingLimit})`,
    )
    err.code = "SESSION_QUEUE_FULL"
    return Promise.reject(err)
  }

  pendingCounts.set(sessionKey, pending + 1)
  const prev = messageInflight.get(sessionKey) || Promise.resolve()
  const next = prev
    .catch(() => {})
    .then(task)
    .finally(() => {
      const remain = (pendingCounts.get(sessionKey) || 1) - 1
      if (remain <= 0) {
        pendingCounts.delete(sessionKey)
      } else {
        pendingCounts.set(sessionKey, remain)
      }
      if (messageInflight.get(sessionKey) === next) {
        messageInflight.delete(sessionKey)
      }
    })

  messageInflight.set(sessionKey, next)
  return next
}

async function handleToolUpdate(part) {
  const update = getCompletedToolUpdate(part)
  if (!update) return

  const session = sessionsById.get(update.sessionId)
  if (!session) return

  const title = update.title
  const tool = update.tool
  await postThreadMessage(session.channel, session.threadTs, `*${tool}* - ${title}`)
}

void runEventSubscriptionLoop({
  label: "slack event subscription",
  subscribe: () =>
    withTimeout(
      (signal) => opencode.client.event.subscribe(undefined, { signal }),
      "slack event.subscribe",
    ),
  onEvent: async (event) => {
    if (event?.type !== "message.part.updated") return
    const part = event?.properties?.part
    await handleToolUpdate(part)
  },
})

app.message(async ({ message, say }) => {
  if (!message || message.subtype || !("text" in message) || !message.text) return

  const channel = message.channel
  const threadTs = message.thread_ts || message.ts
  try {
    await enqueueThreadMessage(channel, threadTs, async () => {
      try {
        await limiter.run(async () => {
          let session
          try {
            session = await ensureSession(channel, threadTs)
          } catch {
            await say({
              text: "Sorry, I had trouble creating a session. Please try again.",
              thread_ts: threadTs,
            })
            return
          }

          let result
          try {
            result = await withTimeout(
              (signal) =>
                opencode.client.session.prompt(
                  {
                    path: { id: session.sessionId },
                    body: { parts: [{ type: "text", text: message.text }] },
                  },
                  { signal },
                ),
              "slack session.prompt",
            )
          } catch {
            await say({
              text: "Sorry, I had trouble processing your message. Please try again.",
              thread_ts: threadTs,
            })
            return
          }

          if (result.error) {
            await say({
              text: "Sorry, I had trouble processing your message. Please try again.",
              thread_ts: threadTs,
            })
            return
          }

          const response = result.data
          const responseText = buildResponseText(response)

          await say({ text: responseText, thread_ts: threadTs })
        })
      } catch {
        await say({
          text: "Sorry, I had trouble processing your message. Please try again.",
          thread_ts: threadTs,
        })
      }
    })
  } catch (err) {
    if (err?.code === "SESSION_QUEUE_FULL") {
      await say({
        text: "Sorry, I'm handling too many messages in this thread. Please try again shortly.",
        thread_ts: threadTs,
      })
      return
    }
    console.error("slack enqueue message failed:", err?.message || err)
  }
})

app.command("/test", async ({ ack, say }) => {
  await ack()
  await say("Bot is working.")
})

await app.start()
console.log("⚡️ Slack bot is running!")
