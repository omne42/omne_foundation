import http from "http"
import * as lark from "@larksuiteoapi/node-sdk"
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

assertEnv("FEISHU_APP_ID")
assertEnv("FEISHU_APP_SECRET")
assertEnv("FEISHU_VERIFICATION_TOKEN")
assertEnv("FEISHU_ENCRYPT_KEY", { optional: true })

function parsePortEnv(name, fallback = "3000") {
  const raw = String(process.env[name] || fallback).trim()
  const value = Number.parseInt(raw, 10)
  if (!Number.isSafeInteger(value) || value < 1 || value > 65535) {
    throw new Error(`invalid ${name}: expected an integer in range 1..65535`)
  }
  return value
}

const port = parsePortEnv("PORT")

console.log("🚀 Starting opencode server...")
const opencode = await createOpencode({ port: 0 })
console.log("✅ Opencode server ready")

const limiter = createBotLimiter()
const store = await createBotSessionStore()

const client = new lark.Client({
  appId: process.env.FEISHU_APP_ID,
  appSecret: process.env.FEISHU_APP_SECRET,
})

/**
 * sessionKey = JSON.stringify([tenantKey ?? "default", chatId])
 * value = { sessionId, tenantKey, chatId }
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

function getSessionKey(tenantKey, chatId) {
  const normalizedTenantKey =
    typeof tenantKey === "string" && tenantKey.trim() !== "" ? tenantKey : "default"
  return JSON.stringify([normalizedTenantKey, String(chatId || "")])
}

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
  const chatId = typeof value.chatId === "string" ? value.chatId : ""
  if (!sessionId || !chatId) continue

  sessionsById.set(sessionId, {
    sessionId,
    tenantKey: typeof value.tenantKey === "string" ? value.tenantKey : null,
    chatId,
  })
}

async function sendTextToChat(tenantKey, chatId, text) {
  if (!chatId || !text) return
  const req = {
    params: { receive_id_type: "chat_id" },
    data: {
      receive_id: chatId,
      msg_type: "text",
      content: JSON.stringify({ text }),
    },
  }

  const tenantOpt =
    tenantKey && String(tenantKey).trim() !== "" ? lark.withTenantKey(tenantKey) : undefined

  await ignoreError(
    withTimeout(client.im.message.create(req, tenantOpt), "feishu im.message.create"),
    "feishu send message failed",
  )
}

async function ensureSession(tenantKey, chatId) {
  const sessionKey = getSessionKey(tenantKey, chatId)
  const existing = sessions.get(sessionKey)
  const existingSessionId =
    typeof existing === "string"
      ? existing
      : typeof existing?.sessionId === "string"
        ? existing.sessionId
        : ""
  if (existingSessionId) {
    const session = { sessionId: existingSessionId, tenantKey, chatId }
    if (
      typeof existing !== "object" ||
      existing === null ||
      existing.sessionId !== existingSessionId ||
      existing.tenantKey !== tenantKey ||
      existing.chatId !== chatId
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
    const created = await withTimeout(
      (signal) =>
        opencode.client.session.create(
          {
            body: { title: `Feishu chat ${chatId}` },
          },
          { signal },
        ),
      "feishu session.create",
    )

    if (created.error) {
      throw new Error(created.error.message || "failed to create session")
    }

    const session = { sessionId: created.data.id, tenantKey, chatId }
    setStoredSession(sessionKey, session)

    let url = null
    try {
      const share = await withTimeout(
        (signal) => opencode.client.session.share({ path: { id: session.sessionId } }, { signal }),
        "feishu session.share",
      )
      url = share?.data?.share?.url || null
    } catch (err) {
      console.error("session share failed:", err?.message || err)
    }
    if (url) {
      await sendTextToChat(tenantKey, chatId, url)
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

async function handleUserText(tenantKey, chatId, text) {
  const trimmed = String(text || "").trim()
  if (!trimmed) return

  if (trimmed === "/test") {
    await sendTextToChat(tenantKey, chatId, "Bot is working.")
    return
  }

  try {
    await limiter.run(async () => {
      let session
      try {
        session = await ensureSession(tenantKey, chatId)
      } catch {
        await sendTextToChat(tenantKey, chatId, "Sorry, I had trouble creating a session.")
        return
      }

      let result
      try {
        result = await withTimeout(
          (signal) =>
            opencode.client.session.prompt(
              {
                path: { id: session.sessionId },
                body: { parts: [{ type: "text", text: trimmed }] },
              },
              { signal },
            ),
          "feishu session.prompt",
        )
      } catch {
        await sendTextToChat(tenantKey, chatId, "Sorry, I had trouble processing your message.")
        return
      }

      if (result.error) {
        await sendTextToChat(tenantKey, chatId, "Sorry, I had trouble processing your message.")
        return
      }

      const response = result.data
      const responseText = buildResponseText(response)

      await sendTextToChat(tenantKey, chatId, responseText)
    })
  } catch {
    await sendTextToChat(tenantKey, chatId, "Sorry, I had trouble processing your message.")
  }
}

function enqueueChatMessage(tenantKey, chatId, text) {
  const sessionKey = getSessionKey(tenantKey, chatId)
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
    .then(() => handleUserText(tenantKey, chatId, text))
    .catch((err) => {
      console.error("handle message failed:", err?.message || err)
    })
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
  await sendTextToChat(
    session.tenantKey,
    session.chatId,
    `${update.tool} - ${update.title}`,
  )
}

void runEventSubscriptionLoop({
  label: "feishu event subscription",
  subscribe: () =>
    withTimeout(
      (signal) => opencode.client.event.subscribe(undefined, { signal }),
      "feishu event.subscribe",
    ),
  onEvent: async (event) => {
    if (event?.type !== "message.part.updated") return
    const part = event?.properties?.part
    await handleToolUpdate(part)
  },
})

const dispatcher = new lark.EventDispatcher({
  encryptKey: process.env.FEISHU_ENCRYPT_KEY,
  verificationToken: process.env.FEISHU_VERIFICATION_TOKEN,
}).register({
  "im.message.receive_v1": async (data) => {
    if (!data || !data.message || !data.sender) return
    if (data.sender.sender_type !== "user") return

    if (data.message.message_type !== "text") return

    const tenantKey = data.tenant_key || data.sender.tenant_key || null
    const chatId = data.message.chat_id
    const content = data.message.content
    if (!chatId || !content) return

    let text
    try {
      text = JSON.parse(content).text
    } catch {
      return
    }

    queueMicrotask(() => {
      enqueueChatMessage(tenantKey, chatId, text).catch((err) => {
        if (err?.code === "SESSION_QUEUE_FULL") {
          sendTextToChat(
            tenantKey,
            chatId,
            "Sorry, I'm handling too many messages in this chat. Please try again shortly.",
          ).catch(() => {})
          return
        }
        console.error("feishu enqueue message failed:", err?.message || err)
      })
    })
  },
})

const server = http.createServer()
server.on(
  "request",
  lark.adaptDefault("/webhook/event", dispatcher, {
    autoChallenge: true,
  }),
)
server.listen(port, () => {
  console.log(`⚡️ Feishu bot is listening on :${port}`)
})
