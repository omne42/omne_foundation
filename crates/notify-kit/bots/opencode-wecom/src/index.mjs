import crypto from "node:crypto"
import http from "node:http"
import { URL } from "node:url"

import { XMLParser } from "fast-xml-parser"
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

assertEnv("WECOM_CORP_ID")
assertEnv("WECOM_CORP_SECRET")
assertEnv("WECOM_AGENT_ID")
assertEnv("WECOM_TOKEN")
assertEnv("WECOM_ENCODING_AES_KEY")

function parsePositiveIntegerEnv(name) {
  const raw = String(process.env[name] || "").trim()
  const value = Number.parseInt(raw, 10)
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new Error(`invalid ${name}: expected a positive integer`)
  }
  return value
}

function parsePortEnv(name, fallback = "3000") {
  const raw = String(process.env[name] || fallback).trim()
  const value = Number.parseInt(raw, 10)
  if (!Number.isSafeInteger(value) || value < 1 || value > 65535) {
    throw new Error(`invalid ${name}: expected an integer in range 1..65535`)
  }
  return value
}

const wecomAgentId = parsePositiveIntegerEnv("WECOM_AGENT_ID")
const port = parsePortEnv("PORT")
const sessionScope = (process.env.WECOM_SESSION_SCOPE || "user").toLowerCase()
const replyTo = (process.env.WECOM_REPLY_TO || "user").toLowerCase()
const botHttpTimeoutMsValue = Number.parseInt(String(process.env.OPENCODE_BOT_HTTP_TIMEOUT_MS || "15000"), 10)
const botHttpTimeoutMs =
  Number.isFinite(botHttpTimeoutMsValue) && botHttpTimeoutMsValue > 0 ? botHttpTimeoutMsValue : 15000

const xml = new XMLParser({ ignoreAttributes: true })

function createFetchTimeoutSignal(timeoutMs) {
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) return undefined
  if (typeof AbortSignal === "undefined" || typeof AbortSignal.timeout !== "function") return undefined
  return AbortSignal.timeout(timeoutMs)
}

function sha1Hex(value) {
  return crypto.createHash("sha1").update(String(value)).digest("hex")
}

function parseSha1HexOrNull(value) {
  const s = String(value || "").trim()
  if (s.length !== 40) return null
  for (let i = 0; i < s.length; i += 1) {
    const c = s.charCodeAt(i)
    const isDigit = c >= 48 && c <= 57
    const isLower = c >= 97 && c <= 102
    const isUpper = c >= 65 && c <= 70
    if (!(isDigit || isLower || isUpper)) return null
  }
  return Buffer.from(s, "hex")
}

function timingSafeEqualSha1Hex(actualHex, expectedHex) {
  const expected = parseSha1HexOrNull(expectedHex)
  const actual = parseSha1HexOrNull(actualHex)
  const expectedBuf = expected || Buffer.alloc(20, 0)
  const actualBuf = actual || Buffer.alloc(20, 0)
  const eq = crypto.timingSafeEqual(actualBuf, expectedBuf)
  return Boolean(actual && expected && eq)
}

function computeSignature(token, timestamp, nonce, encrypted) {
  const items = [token, timestamp, nonce, encrypted].map((v) => String(v || ""))
  items.sort()
  return sha1Hex(items.join(""))
}

function decodeAesKey(encodingAesKey) {
  // WeCom provides 43 chars base64; add '=' padding to make it valid base64.
  const key = Buffer.from(`${encodingAesKey}=`, "base64")
  if (key.length !== 32) throw new Error("invalid WECOM_ENCODING_AES_KEY (expected 32 bytes after base64 decode)")
  return key
}

function pkcs7Unpad(buf) {
  if (!buf || buf.length === 0) throw new Error("invalid pkcs7 padding")
  const pad = buf[buf.length - 1]
  if (pad < 1 || pad > 32) throw new Error("invalid pkcs7 padding length")
  for (let i = 1; i <= pad; i += 1) {
    if (buf[buf.length - i] !== pad) throw new Error("invalid pkcs7 padding")
  }
  return buf.subarray(0, buf.length - pad)
}

function decryptWeCom(encryptedBase64, encodingAesKey) {
  const aesKey = decodeAesKey(encodingAesKey)
  const iv = aesKey.subarray(0, 16)
  const cipherText = Buffer.from(String(encryptedBase64 || ""), "base64")

  const decipher = crypto.createDecipheriv("aes-256-cbc", aesKey, iv)
  decipher.setAutoPadding(false)
  let plain = Buffer.concat([decipher.update(cipherText), decipher.final()])
  plain = pkcs7Unpad(plain)

  if (plain.length < 20) throw new Error("invalid decrypted message")
  const msgLen = plain.readUInt32BE(16)
  const msgStart = 20
  const msgEnd = msgStart + msgLen
  if (msgEnd > plain.length) throw new Error("invalid decrypted message")
  const xmlText = plain.subarray(msgStart, msgEnd).toString("utf-8")
  const receiver = plain.subarray(msgEnd).toString("utf-8").replace(/\0+$/u, "")

  return { xmlText, receiver }
}

function assertReceiverOrThrow(receiver) {
  const expected = String(process.env.WECOM_CORP_ID || "").trim()
  const actual = String(receiver || "").trim()
  if (!expected || !actual || expected !== actual) {
    throw new Error("invalid receiver corp id")
  }
}

async function readRequestBody(req, { limitBytes = 1024 * 1024 } = {}) {
  const chunks = []
  let size = 0
  for await (const chunk of req) {
    size += chunk.length
    if (size > limitBytes) throw new Error("request body too large")
    chunks.push(chunk)
  }
  return Buffer.concat(chunks).toString("utf-8")
}

let accessTokenCache = null
let accessTokenExpiresAtMs = 0
let accessTokenRefreshInflight = null

async function getWeComAccessToken() {
  const now = Date.now()
  if (accessTokenCache && now < accessTokenExpiresAtMs) return accessTokenCache
  if (accessTokenRefreshInflight) return accessTokenRefreshInflight

  const refreshing = (async () => {
    const corpId = process.env.WECOM_CORP_ID
    const corpSecret = process.env.WECOM_CORP_SECRET
    const url = new URL("https://qyapi.weixin.qq.com/cgi-bin/gettoken")
    url.searchParams.set("corpid", corpId)
    url.searchParams.set("corpsecret", corpSecret)

    const resp = await fetch(url, {
      method: "GET",
      signal: createFetchTimeoutSignal(botHttpTimeoutMs),
    })
    const data = await resp.json().catch(() => null)
    if (!resp.ok || !data || data.errcode) {
      throw new Error(`wecom gettoken failed: ${data?.errmsg || resp.status}`)
    }

    accessTokenCache = data.access_token
    const expiresInSec = Number.parseInt(String(data.expires_in || "7200"), 10)
    accessTokenExpiresAtMs = Date.now() + Math.max(60, expiresInSec - 120) * 1000
    return accessTokenCache
  })()

  accessTokenRefreshInflight = refreshing
  try {
    return await refreshing
  } finally {
    if (accessTokenRefreshInflight === refreshing) {
      accessTokenRefreshInflight = null
    }
  }
}

async function wecomPost(path, body) {
  const token = await getWeComAccessToken()
  const url = new URL(`https://qyapi.weixin.qq.com/cgi-bin/${path}`)
  url.searchParams.set("access_token", token)

  const resp = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
    signal: createFetchTimeoutSignal(botHttpTimeoutMs),
  })
  const data = await resp.json().catch(() => null)
  if (!resp.ok || !data || data.errcode) {
    throw new Error(`wecom api failed (${path}): ${data?.errmsg || resp.status}`)
  }
  return data
}

async function sendTextToUser(userId, text) {
  if (!userId || !text) return
  await ignoreError(
    wecomPost("message/send", {
      touser: userId,
      msgtype: "text",
      agentid: wecomAgentId,
      text: { content: text },
      safe: 0,
    }),
    "wecom sendTextToUser failed",
  )
}

async function sendTextToChat(chatId, text) {
  if (!chatId || !text) return
  await ignoreError(
    wecomPost("appchat/send", {
      chatid: chatId,
      msgtype: "text",
      text: { content: text },
    }),
    "wecom sendTextToChat failed",
  )
}

async function sendText({ userId, chatId }, text) {
  if (!text) return
  if (replyTo === "chat" && chatId) {
    await sendTextToChat(chatId, text)
    return
  }
  await sendTextToUser(userId, text)
}

console.log("🚀 Starting opencode server...")
const opencode = await createOpencode({ port: 0 })
console.log("✅ Opencode server ready")

const limiter = createBotLimiter()
const store = await createBotSessionStore()

/**
 * sessionKey = `${scope}-${id}`
 * value = { sessionId, userId, chatId }
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
  const userId = typeof value.userId === "string" ? value.userId : ""
  const chatId = typeof value.chatId === "string" ? value.chatId : null
  if (!sessionId || !userId) continue
  sessionsById.set(sessionId, { sessionId, userId, chatId })
}

function getSessionKey({ userId, chatId }) {
  if (sessionScope === "chat" && chatId) return `chat-${chatId}`
  return `user-${userId}`
}

function enqueueSessionMessage(ctx, text) {
  const sessionKey = getSessionKey(ctx)
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
    .then(() => handleUserText(ctx, text))
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

async function ensureSession(ctx) {
  const key = getSessionKey(ctx)
  const existing = sessions.get(key)
  const existingSessionId =
    typeof existing === "string"
      ? existing
      : typeof existing?.sessionId === "string"
        ? existing.sessionId
        : ""
  if (existingSessionId) {
    const session = {
      sessionId: existingSessionId,
      userId: ctx.userId,
      chatId: ctx.chatId || null,
    }
    if (
      typeof existing !== "object" ||
      existing === null ||
      existing.sessionId !== existingSessionId ||
      existing.userId !== session.userId ||
      existing.chatId !== session.chatId
    ) {
      setStoredSession(key, session)
    } else {
      sessionsById.set(existingSessionId, session)
    }
    return session
  }

  const inflight = sessionCreateInflight.get(key)
  if (inflight) {
    return inflight
  }

  const creating = (async () => {
    const created = await withTimeout(
      (signal) =>
        opencode.client.session.create(
          {
            body: { title: `WeCom ${key}` },
          },
          { signal },
        ),
      "wecom session.create",
    )
    if (created.error) {
      throw new Error(created.error.message || "failed to create session")
    }

    const session = { sessionId: created.data.id, userId: ctx.userId, chatId: ctx.chatId || null }
    setStoredSession(key, session)

    let url = null
    try {
      const share = await withTimeout(
        (signal) => opencode.client.session.share({ path: { id: session.sessionId } }, { signal }),
        "wecom session.share",
      )
      url = share?.data?.share?.url || null
    } catch (err) {
      console.error("session share failed:", err?.message || err)
    }
    if (url) {
      await sendText(session, url)
    }

    return session
  })()

  sessionCreateInflight.set(key, creating)
  try {
    return await creating
  } finally {
    if (sessionCreateInflight.get(key) === creating) {
      sessionCreateInflight.delete(key)
    }
  }
}

async function handleUserText(ctx, text) {
  const trimmed = String(text || "").trim()
  if (!trimmed) return

  if (trimmed === "/test") {
    await sendText(ctx, "Bot is working.")
    return
  }

  try {
    await limiter.run(async () => {
      let session
      try {
        session = await ensureSession(ctx)
      } catch {
        await sendText(ctx, "Sorry, I had trouble creating a session.")
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
          "wecom session.prompt",
        )
      } catch {
        await sendText(ctx, "Sorry, I had trouble processing your message.")
        return
      }

      if (result.error) {
        await sendText(ctx, "Sorry, I had trouble processing your message.")
        return
      }

      const responseText = buildResponseText(result.data)

      await sendText(ctx, responseText)
    })
  } catch {
    await sendText(ctx, "Sorry, I had trouble processing your message.")
  }
}

async function handleToolUpdate(part) {
  const update = getCompletedToolUpdate(part)
  if (!update) return

  const session = sessionsById.get(update.sessionId)
  if (!session) return
  await sendText(session, `${update.tool} - ${update.title}`)
}

void runEventSubscriptionLoop({
  label: "wecom event subscription",
  subscribe: () =>
    withTimeout(
      (signal) => opencode.client.event.subscribe(undefined, { signal }),
      "wecom event.subscribe",
    ),
  onEvent: async (event) => {
    if (event?.type !== "message.part.updated") return
    const part = event?.properties?.part
    await handleToolUpdate(part)
  },
})

function parseWeComEncryptedXml(encryptedXmlText) {
  const parsed = xml.parse(encryptedXmlText)
  const root = parsed?.xml || parsed
  const encrypt = root?.Encrypt
  return String(encrypt || "")
}

function parseWeComPlainXml(plainXmlText) {
  const parsed = xml.parse(plainXmlText)
  const root = parsed?.xml || parsed
  return {
    toUserName: root?.ToUserName || null,
    fromUserName: root?.FromUserName || null,
    agentId: root?.AgentID || null,
    msgType: root?.MsgType || null,
    content: root?.Content || null,
    chatId: root?.ChatId || null,
  }
}

function verifySignatureOrThrow({ signature, timestamp, nonce, encrypted }) {
  const token = process.env.WECOM_TOKEN
  const expected = computeSignature(token, timestamp, nonce, encrypted)
  if (!timingSafeEqualSha1Hex(signature, expected)) {
    throw new Error("invalid msg_signature")
  }
}

const REPLAY_WINDOW_SECONDS = 5 * 60
const REPLAY_CACHE_TTL_MS = 10 * 60 * 1000
const REPLAY_CACHE_MAX_ENTRIES = 10_000
const REPLAY_CLEANUP_INTERVAL_MS = 30 * 1000
const REPLAY_NONCE_MAX_BYTES = 128
const replayCache = new Map()
let replayLastCleanupMs = 0

function cleanupReplayCache(now) {
  if (now - replayLastCleanupMs < REPLAY_CLEANUP_INTERVAL_MS) return
  replayLastCleanupMs = now

  // Entries are inserted with a fixed TTL; insertion order approximates expiration order.
  for (const [k, exp] of replayCache.entries()) {
    if (exp > now) break
    replayCache.delete(k)
  }

  while (replayCache.size > REPLAY_CACHE_MAX_ENTRIES) {
    const oldest = replayCache.keys().next().value
    if (oldest === undefined) break
    replayCache.delete(oldest)
  }
}

function normalizeTimestampSeconds(timestamp) {
  const raw = String(timestamp ?? "").trim()
  if (!/^[0-9]+$/.test(raw)) return null
  const normalized = raw.replace(/^0+(?=\d)/, "")
  const value = Number.parseInt(normalized, 10)
  if (!Number.isSafeInteger(value) || value <= 0) return null
  return { value, key: String(value) }
}

function isFreshTimestamp(seconds) {
  const now = Math.floor(Date.now() / 1000)
  return Math.abs(now - seconds) <= REPLAY_WINDOW_SECONDS
}

function isValidReplayNonce(nonce) {
  const value = String(nonce ?? "")
  const size = Buffer.byteLength(value, "utf8")
  return size > 0 && size <= REPLAY_NONCE_MAX_BYTES
}

function checkAndRememberReplay(timestamp, nonce) {
  const key = `${timestamp}:${nonce}`
  const now = Date.now()

  cleanupReplayCache(now)

  if (replayCache.has(key)) {
    return false
  }

  replayCache.set(key, now + REPLAY_CACHE_TTL_MS)
  while (replayCache.size > REPLAY_CACHE_MAX_ENTRIES) {
    const oldest = replayCache.keys().next().value
    if (oldest === undefined) break
    replayCache.delete(oldest)
  }
  return true
}

function sendTextResponse(res, status, body) {
  res.statusCode = status
  res.setHeader("content-type", "text/plain; charset=utf-8")
  res.end(body)
}

const server = http.createServer(async (req, res) => {
  try {
    // Parse request target against a trusted base URL; never trust the Host header here.
    const url = new URL(String(req.url || "/"), "http://localhost")

    if (url.pathname !== "/webhook/wecom") {
      sendTextResponse(res, 404, "not found")
      return
    }

    if (req.method === "GET") {
      const signature = url.searchParams.get("msg_signature")
      const timestamp = url.searchParams.get("timestamp")
      const nonce = url.searchParams.get("nonce")
      const echostr = url.searchParams.get("echostr")

      if (!signature || !timestamp || !nonce || !echostr) {
        sendTextResponse(res, 400, "missing query params")
        return
      }

      try {
        verifySignatureOrThrow({ signature, timestamp, nonce, encrypted: echostr })
        const { xmlText, receiver } = decryptWeCom(echostr, process.env.WECOM_ENCODING_AES_KEY)
        assertReceiverOrThrow(receiver)
        sendTextResponse(res, 200, xmlText)
      } catch (err) {
        console.error("wecom verify failed:", err?.message || err)
        sendTextResponse(res, 403, "forbidden")
      }
      return
    }

    if (req.method === "POST") {
      const signature = url.searchParams.get("msg_signature")
      const timestamp = url.searchParams.get("timestamp")
      const nonce = url.searchParams.get("nonce")
      if (!signature || !timestamp || !nonce) {
        sendTextResponse(res, 400, "missing query params")
        return
      }

      let rawBody
      try {
        rawBody = await readRequestBody(req)
      } catch (err) {
        console.error("wecom read body failed:", err?.message || err)
        sendTextResponse(res, 413, "payload too large")
        return
      }

      const encrypted = parseWeComEncryptedXml(rawBody)
      if (!encrypted) {
        sendTextResponse(res, 400, "invalid payload")
        return
      }
      const normalizedTimestamp = normalizeTimestampSeconds(timestamp)
      if (!normalizedTimestamp || !isFreshTimestamp(normalizedTimestamp.value)) {
        sendTextResponse(res, 403, "forbidden")
        return
      }
      if (!isValidReplayNonce(nonce)) {
        sendTextResponse(res, 403, "forbidden")
        return
      }
      try {
        verifySignatureOrThrow({ signature, timestamp, nonce, encrypted })
      } catch (err) {
        console.error("wecom verify failed:", err?.message || err)
        sendTextResponse(res, 403, "forbidden")
        return
      }
      if (!checkAndRememberReplay(normalizedTimestamp.key, nonce)) {
        sendTextResponse(res, 403, "forbidden")
        return
      }

      let msg
      try {
        const { xmlText, receiver } = decryptWeCom(encrypted, process.env.WECOM_ENCODING_AES_KEY)
        assertReceiverOrThrow(receiver)
        msg = parseWeComPlainXml(xmlText)
      } catch (err) {
        console.error("wecom decrypt failed:", err?.message || err)
        sendTextResponse(res, 403, "forbidden")
        return
      }

      sendTextResponse(res, 200, "success")

      queueMicrotask(() => {
        const parsedAgentId = Number.parseInt(String(msg.agentId || ""), 10)
        if (!Number.isSafeInteger(parsedAgentId) || parsedAgentId !== wecomAgentId) {
          return
        }

        const userId = msg.fromUserName
        const chatId = msg.chatId
        const msgType = msg.msgType
        const content = msg.content

        if (!userId) return
        if (msgType !== "text") return

        const ctx = { userId, chatId }
        enqueueSessionMessage(ctx, content).catch((err) => {
          if (err?.code === "SESSION_QUEUE_FULL") {
            sendText(
              ctx,
              "Sorry, I'm handling too many messages in this session. Please try again shortly.",
            ).catch(() => {})
            return
          }
          console.error("wecom enqueue message failed:", err?.message || err)
        })
      })

      return
    }

    sendTextResponse(res, 405, "method not allowed")
  } catch (err) {
    console.error("wecom request handling failed:", err?.message || err)
    if (!res.headersSent) {
      sendTextResponse(res, 400, "bad request")
      return
    }
    res.destroy(err instanceof Error ? err : undefined)
  }
})

server.listen(port, () => {
  console.log(`⚡️ WeCom bot is listening on :${port} (/webhook/wecom)`)
})
