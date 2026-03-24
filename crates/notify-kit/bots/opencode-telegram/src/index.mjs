import process from "node:process"

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

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

function truncateForTelegram(text, max = 3800) {
  const s = String(text || "")
  if (s.length <= max) return s
  return `${s.slice(0, max - 20)}\n\n[truncated]\n`
}

const token = assertEnv("TELEGRAM_BOT_TOKEN")
const apiBase = `https://api.telegram.org/bot${token}`
const botHttpTimeoutMsValue = Number.parseInt(String(process.env.OPENCODE_BOT_HTTP_TIMEOUT_MS || "15000"), 10)
const botHttpTimeoutMs =
  Number.isFinite(botHttpTimeoutMsValue) && botHttpTimeoutMsValue > 0 ? botHttpTimeoutMsValue : 15000

function createFetchTimeoutSignal(timeoutMs) {
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) return undefined
  if (typeof AbortSignal === "undefined" || typeof AbortSignal.timeout !== "function") return undefined
  return AbortSignal.timeout(timeoutMs)
}

async function tg(method, payload) {
  const longPollSeconds = Number.parseInt(String(payload?.timeout || "0"), 10)
  const timeoutMs =
    Number.isFinite(longPollSeconds) && longPollSeconds > 0
      ? Math.max(botHttpTimeoutMs, (longPollSeconds + 15) * 1000)
      : botHttpTimeoutMs
  const resp = await fetch(`${apiBase}/${method}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(payload ?? {}),
    signal: createFetchTimeoutSignal(timeoutMs),
  })

  const data = await resp.json().catch(() => null)
  if (!resp.ok || !data?.ok) {
    const desc = data?.description || `http ${resp.status}`
    throw new Error(`telegram api error: ${method}: ${desc}`)
  }

  return data.result
}

async function sendMessage(chatId, text) {
  await ignoreError(
    tg("sendMessage", { chat_id: chatId, text: truncateForTelegram(text) }),
    "telegram sendMessage failed",
  )
}

console.log("🚀 Starting opencode server...")
const opencode = await createOpencode({ port: 0 })
console.log("✅ Opencode server ready")

const limiter = createBotLimiter()
const store = await createBotSessionStore()

/**
 * chatId -> sessionId
 */
const chatToSession = store.map
/**
 * sessionId -> chatId
 */
const sessionToChat = new Map()
const sessionCreateInflight = new Map()
const chatMessageInflight = new Map()
const chatPendingCounts = new Map()
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
      sessionToChat.delete(evictedSessionId)
    }
  }
}

function setStoredSession(chatId, sessionId) {
  const previousSessionId = getStoredSessionId(chatToSession.get(chatId))
  if (previousSessionId && previousSessionId !== sessionId) {
    sessionToChat.delete(previousSessionId)
  }
  const evicted = store.set(chatId, sessionId)
  sessionToChat.set(sessionId, chatId)
  applyStoreEvictions(evicted)
}

for (const [chatId, value] of chatToSession.entries()) {
  if (!chatId) continue
  const sessionId = getStoredSessionId(value)
  if (!sessionId) continue
  if (typeof value !== "string") {
    setStoredSession(chatId, sessionId)
    continue
  }
  sessionToChat.set(sessionId, chatId)
}

async function ensureSession(chatId) {
  const existing = chatToSession.get(chatId)
  const existingSessionId = getStoredSessionId(existing)
  if (existingSessionId) {
    if (typeof existing !== "string") {
      setStoredSession(chatId, existingSessionId)
    } else {
      sessionToChat.set(existingSessionId, chatId)
    }
    return { chatId, sessionId: existingSessionId }
  }

  const inflight = sessionCreateInflight.get(chatId)
  if (inflight) {
    return inflight
  }

  const creating = (async () => {
    const created = await withTimeout(
      (signal) =>
        opencode.client.session.create(
          {
            body: { title: `Telegram chat ${chatId}` },
          },
          { signal },
        ),
      "telegram session.create",
    )
    if (created.error) {
      throw new Error(created.error.message || "failed to create session")
    }

    const sessionId = created.data.id
    setStoredSession(chatId, sessionId)

    let url = null
    try {
      const share = await withTimeout(
        (signal) => opencode.client.session.share({ path: { id: sessionId } }, { signal }),
        "telegram session.share",
      )
      url = share?.data?.share?.url || null
    } catch (err) {
      console.error("session share failed:", err?.message || err)
    }
    if (url) {
      await sendMessage(chatId, url)
    }

    return { chatId, sessionId }
  })()

  sessionCreateInflight.set(chatId, creating)
  try {
    return await creating
  } finally {
    if (sessionCreateInflight.get(chatId) === creating) {
      sessionCreateInflight.delete(chatId)
    }
  }
}

async function handleUserText(chatId, text) {
  const trimmed = String(text || "").trim()
  if (!trimmed) return

  if (trimmed === "/test") {
    await sendMessage(chatId, "Bot is working.")
    return
  }

  try {
    await limiter.run(async () => {
      let session
      try {
        session = await ensureSession(chatId)
      } catch {
        await sendMessage(chatId, "Sorry, I had trouble creating a session.")
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
          "telegram session.prompt",
        )
      } catch {
        await sendMessage(chatId, "Sorry, I had trouble processing your message.")
        return
      }

      if (result.error) {
        await sendMessage(chatId, "Sorry, I had trouble processing your message.")
        return
      }

      const responseText = buildResponseText(result.data)
      await sendMessage(chatId, responseText)
    })
  } catch {
    await sendMessage(chatId, "Sorry, I had trouble processing your message.")
  }
}

function enqueueChatMessage(chatId, text) {
  const pending = chatPendingCounts.get(chatId) || 0
  if (pending >= sessionPendingLimit) {
    const err = new Error(`session queue is full (chatId=${chatId}, maxPending=${sessionPendingLimit})`)
    err.code = "SESSION_QUEUE_FULL"
    return Promise.reject(err)
  }

  chatPendingCounts.set(chatId, pending + 1)
  const prev = chatMessageInflight.get(chatId) || Promise.resolve()
  const next = prev
    .catch(() => {})
    .then(() => handleUserText(chatId, text))
    .catch((err) => {
      console.error("telegram handle message failed:", err?.message || err)
    })
    .finally(() => {
      const remain = (chatPendingCounts.get(chatId) || 1) - 1
      if (remain <= 0) {
        chatPendingCounts.delete(chatId)
      } else {
        chatPendingCounts.set(chatId, remain)
      }
      if (chatMessageInflight.get(chatId) === next) {
        chatMessageInflight.delete(chatId)
      }
    })

  chatMessageInflight.set(chatId, next)
  return next
}

async function handleToolUpdate(part) {
  const update = getCompletedToolUpdate(part)
  if (!update) return

  const chatId = sessionToChat.get(update.sessionId)
  if (!chatId) return

  await sendMessage(chatId, `${update.tool} - ${update.title}`)
}

void runEventSubscriptionLoop({
  label: "telegram event subscription",
  subscribe: () =>
    withTimeout(
      (signal) => opencode.client.event.subscribe(undefined, { signal }),
      "telegram event.subscribe",
    ),
  onEvent: async (event) => {
    if (event?.type !== "message.part.updated") return
    const part = event?.properties?.part
    await handleToolUpdate(part)
  },
})

let offset = 0
for (;;) {
  try {
    const updates = await tg("getUpdates", {
      timeout: 30,
      offset,
      allowed_updates: ["message"],
    })

    if (Array.isArray(updates)) {
      for (const update of updates) {
        offset = Math.max(offset, Number(update.update_id || 0) + 1)
        const msg = update.message
        if (!msg || !msg.text) continue
        if (msg.from?.is_bot) continue

        const chatId = String(msg.chat?.id || "")
        if (!chatId) continue

        enqueueChatMessage(chatId, msg.text).catch((err) => {
          if (err?.code === "SESSION_QUEUE_FULL") {
            sendMessage(
              chatId,
              "Sorry, I'm handling too many messages in this chat. Please try again shortly.",
            ).catch(() => {})
            return
          }
          console.error("telegram enqueue message failed:", err?.message || err)
        })
      }
    }
  } catch (err) {
    console.error("telegram poll failed:", err?.message || err)
    await sleep(1000)
  }
}
