import { createOpencode } from "@opencode-ai/sdk"
import { DWClient, DWClientDownStream, EventAck, TOPIC_ROBOT } from "dingtalk-stream"

import { createBotLimiter, createBotSessionStore } from "../../_shared/bootstrap.mjs"
import { ignoreError } from "../../_shared/log.mjs"
import {
  assertEnv,
  buildResponseText,
  getCompletedToolUpdate,
  runEventSubscriptionLoop,
  withTimeout,
} from "../../_shared/opencode.mjs"

assertEnv("DINGTALK_CLIENT_ID")
assertEnv("DINGTALK_CLIENT_SECRET")

console.log("🚀 Starting opencode server...")
const opencode = await createOpencode({ port: 0 })
console.log("✅ Opencode server ready")

const limiter = createBotLimiter()
const store = await createBotSessionStore()

const client = new DWClient({
  clientId: process.env.DINGTALK_CLIENT_ID,
  clientSecret: process.env.DINGTALK_CLIENT_SECRET,
})

/**
 * sessionKey = sessionWebhook
 * value = { sessionId, sessionWebhook }
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
const botHttpTimeoutMsValue = Number.parseInt(String(process.env.OPENCODE_BOT_HTTP_TIMEOUT_MS || "15000"), 10)
const botHttpTimeoutMs =
  Number.isFinite(botHttpTimeoutMsValue) && botHttpTimeoutMsValue > 0 ? botHttpTimeoutMsValue : 15000

function createFetchTimeoutSignal(timeoutMs) {
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) return undefined
  if (typeof AbortSignal === "undefined" || typeof AbortSignal.timeout !== "function") return undefined
  return AbortSignal.timeout(timeoutMs)
}

async function readResponseTextLimited(resp, maxBytes = 16 * 1024) {
  const limit = Number.isFinite(maxBytes) && maxBytes > 0 ? Math.trunc(maxBytes) : 16 * 1024
  const body = resp?.body
  if (!body || typeof body.getReader !== "function") {
    const contentLengthRaw = resp?.headers?.get?.("content-length") || ""
    const contentLength = Number.parseInt(String(contentLengthRaw), 10)
    if (Number.isFinite(contentLength) && contentLength > limit) {
      return "[truncated]"
    }
    if (!Number.isFinite(contentLength)) {
      if (typeof body?.cancel === "function") {
        await body.cancel().catch(() => {})
      }
      return "[omitted: unknown length]"
    }
    const text = await resp.text().catch(() => "")
    if (text.length <= limit) return text
    return `${text.slice(0, limit)}\n[truncated]`
  }

  const reader = body.getReader()
  const decoder = new TextDecoder()
  let bytesRead = 0
  let text = ""
  let truncated = false

  try {
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      const chunk = value instanceof Uint8Array ? value : new Uint8Array(value)
      if (bytesRead >= limit) {
        truncated = true
        break
      }

      const remaining = limit - bytesRead
      if (chunk.byteLength > remaining) {
        text += decoder.decode(chunk.subarray(0, remaining), { stream: true })
        bytesRead += remaining
        truncated = true
        break
      }

      text += decoder.decode(chunk, { stream: true })
      bytesRead += chunk.byteLength
    }
    text += decoder.decode()
  } finally {
    if (truncated && typeof reader.cancel === "function") {
      await reader.cancel().catch(() => {})
    } else if (typeof reader.releaseLock === "function") {
      reader.releaseLock()
    }
  }

  if (!truncated) return text
  return `${text}\n[truncated]`
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

for (const [sessionWebhook, value] of sessions.entries()) {
  const validWebhook = validateSessionWebhook(sessionWebhook)
  if (!validWebhook) continue
  const sessionId =
    typeof value === "string"
      ? value
      : typeof value?.sessionId === "string"
        ? value.sessionId
        : ""
  if (!sessionId) continue
  sessionsById.set(sessionId, { sessionId, sessionWebhook: validWebhook })
}

function validateSessionWebhook(sessionWebhook) {
  let url
  try {
    url = new URL(String(sessionWebhook || ""))
  } catch {
    return null
  }

  if (url.protocol !== "https:") return null
  if (url.username || url.password) return null
  if (url.port && url.port !== "443") return null

  const host = url.hostname.toLowerCase()
  const isDingTalkHost =
    host === "dingtalk.com" ||
    host.endsWith(".dingtalk.com") ||
    host === "dingtalk.cn" ||
    host.endsWith(".dingtalk.cn")
  if (!isDingTalkHost) return null

  return url.toString()
}

async function postSessionMessage(sessionWebhook, text) {
  const accessToken = await client.getAccessToken()
  await ignoreError(
    (async () => {
      const resp = await fetch(sessionWebhook, {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "x-acs-dingtalk-access-token": accessToken,
        },
        body: JSON.stringify({
          msgtype: "text",
          text: { content: text },
        }),
        signal: createFetchTimeoutSignal(botHttpTimeoutMs),
      })

      if (resp.ok) return

      const body = await readResponseTextLimited(resp, 16 * 1024)
      const summary = String(body || "").replace(/\s+/gu, " ").trim().slice(0, 200)
      throw new Error(
        `dingtalk send message failed: http ${resp.status}${summary ? ` ${summary}` : ""}`,
      )
    })(),
    "dingtalk send message failed",
  )
}

async function ensureSession(sessionWebhook) {
  const sessionKey = sessionWebhook
  const existing = sessions.get(sessionKey)
  const existingSessionId =
    typeof existing === "string"
      ? existing
      : typeof existing?.sessionId === "string"
        ? existing.sessionId
        : ""
  if (existingSessionId) {
    const session = { sessionId: existingSessionId, sessionWebhook }
    if (
      typeof existing !== "object" ||
      existing === null ||
      existing.sessionId !== existingSessionId ||
      existing.sessionWebhook !== sessionWebhook
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
            body: { title: "DingTalk session" },
          },
          { signal },
        ),
      "dingtalk session.create",
    )
    if (created.error) {
      throw new Error(created.error.message || "failed to create session")
    }

    const session = { sessionId: created.data.id, sessionWebhook }
    setStoredSession(sessionKey, session)

    let url = null
    try {
      const share = await withTimeout(
        (signal) => opencode.client.session.share({ path: { id: session.sessionId } }, { signal }),
        "dingtalk session.share",
      )
      url = share?.data?.share?.url || null
    } catch (err) {
      console.error("session share failed:", err?.message || err)
    }
    if (url) {
      await postSessionMessage(sessionWebhook, url)
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

async function handleUserText(sessionWebhook, text) {
  const trimmed = String(text || "").trim()
  if (!trimmed) return

  if (trimmed === "/test") {
    await postSessionMessage(sessionWebhook, "Bot is working.")
    return
  }

  try {
    await limiter.run(async () => {
      let session
      try {
        session = await ensureSession(sessionWebhook)
      } catch {
        await postSessionMessage(sessionWebhook, "Sorry, I had trouble creating a session.")
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
          "dingtalk session.prompt",
        )
      } catch {
        await postSessionMessage(sessionWebhook, "Sorry, I had trouble processing your message.")
        return
      }

      if (result.error) {
        await postSessionMessage(sessionWebhook, "Sorry, I had trouble processing your message.")
        return
      }

      const responseText = buildResponseText(result.data)

      await postSessionMessage(sessionWebhook, responseText)
    })
  } catch {
    await postSessionMessage(sessionWebhook, "Sorry, I had trouble processing your message.")
  }
}

function enqueueSessionWebhookMessage(sessionWebhook, text) {
  const pending = pendingCounts.get(sessionWebhook) || 0
  if (pending >= sessionPendingLimit) {
    const err = new Error(
      `session queue is full (sessionWebhook=${sessionWebhook}, maxPending=${sessionPendingLimit})`,
    )
    err.code = "SESSION_QUEUE_FULL"
    return Promise.reject(err)
  }

  pendingCounts.set(sessionWebhook, pending + 1)
  const prev = messageInflight.get(sessionWebhook) || Promise.resolve()
  const next = prev
    .catch(() => {})
    .then(() => handleUserText(sessionWebhook, text))
    .catch((err) => {
      console.error("handle message failed:", err?.message || err)
    })
    .finally(() => {
      const remain = (pendingCounts.get(sessionWebhook) || 1) - 1
      if (remain <= 0) {
        pendingCounts.delete(sessionWebhook)
      } else {
        pendingCounts.set(sessionWebhook, remain)
      }
      if (messageInflight.get(sessionWebhook) === next) {
        messageInflight.delete(sessionWebhook)
      }
    })

  messageInflight.set(sessionWebhook, next)
  return next
}

async function handleToolUpdate(part) {
  const update = getCompletedToolUpdate(part)
  if (!update) return

  const session = sessionsById.get(update.sessionId)
  if (!session) return
  await postSessionMessage(session.sessionWebhook, `${update.tool} - ${update.title}`)
}

void runEventSubscriptionLoop({
  label: "dingtalk event subscription",
  subscribe: () =>
    withTimeout(
      (signal) => opencode.client.event.subscribe(undefined, { signal }),
      "dingtalk event.subscribe",
    ),
  onEvent: async (event) => {
    if (event?.type !== "message.part.updated") return
    const part = event?.properties?.part
    await handleToolUpdate(part)
  },
})

const downstream = new DWClientDownStream(client)
downstream.registerCallbackListener(TOPIC_ROBOT, (res) => {
  const sessionWebhook = validateSessionWebhook(res?.data?.sessionWebhook)
  const content = res?.data?.text?.content
  if (!sessionWebhook || !content) return EventAck.SUCCESS

  queueMicrotask(() => {
    enqueueSessionWebhookMessage(sessionWebhook, content).catch((err) => {
      if (err?.code === "SESSION_QUEUE_FULL") {
        postSessionMessage(
          sessionWebhook,
          "Sorry, I'm handling too many messages in this session. Please try again shortly.",
        ).catch(() => {})
        return
      }
      console.error("dingtalk enqueue message failed:", err?.message || err)
    })
  })

  return EventAck.SUCCESS
})

await downstream.connect()
console.log("⚡️ DingTalk Stream bot is running!")
