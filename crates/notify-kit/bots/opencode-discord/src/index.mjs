import process from "node:process"

import { createOpencode } from "@opencode-ai/sdk"
import { Client, GatewayIntentBits, Partials } from "discord.js"

import { createBotLimiter, createBotSessionStore } from "../../_shared/bootstrap.mjs"
import { ignoreError } from "../../_shared/log.mjs"
import {
  assertEnv,
  buildResponseText,
  getCompletedToolUpdate,
  runEventSubscriptionLoop,
  withTimeout,
} from "../../_shared/opencode.mjs"

function truncateForDiscord(text, max = 1900) {
  const s = String(text || "")
  if (s.length <= max) return s
  return `${s.slice(0, max - 20)}\n\n[truncated]\n`
}

const discordToken = assertEnv("DISCORD_BOT_TOKEN")

console.log("🚀 Starting opencode server...")
const opencode = await createOpencode({ port: 0 })
console.log("✅ Opencode server ready")

const limiter = createBotLimiter()
const store = await createBotSessionStore()

/**
 * channelId -> sessionId
 */
const channelToSession = store.map
/**
 * sessionId -> channelId
 */
const sessionToChannel = new Map()
const sessionCreateInflight = new Map()
const channelMessageInflight = new Map()
const channelPendingCounts = new Map()
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
      sessionToChannel.delete(evictedSessionId)
    }
  }
}

function setStoredSession(channelId, sessionId) {
  const previousSessionId = getStoredSessionId(channelToSession.get(channelId))
  if (previousSessionId && previousSessionId !== sessionId) {
    sessionToChannel.delete(previousSessionId)
  }
  const evicted = store.set(channelId, sessionId)
  sessionToChannel.set(sessionId, channelId)
  applyStoreEvictions(evicted)
}

for (const [channelId, value] of channelToSession.entries()) {
  const sessionId = getStoredSessionId(value)
  if (sessionId) {
    if (typeof value !== "string") {
      setStoredSession(channelId, sessionId)
      continue
    }
    sessionToChannel.set(sessionId, channelId)
  }
}

const client = new Client({
  intents: [
    GatewayIntentBits.Guilds,
    GatewayIntentBits.GuildMessages,
    GatewayIntentBits.DirectMessages,
    GatewayIntentBits.MessageContent,
  ],
  partials: [Partials.Channel],
})

async function postChannelMessage(channelId, text) {
  let channel = client.channels.cache.get(channelId)
  if (!channel || !channel.isTextBased()) {
    channel = await ignoreError(
      withTimeout(client.channels.fetch(channelId), "discord channels.fetch"),
      "discord fetch channel failed",
    )
  }
  if (!channel || !channel.isTextBased()) return
  await ignoreError(
    withTimeout(channel.send(truncateForDiscord(text)), "discord channel.send"),
    "discord channel send failed",
  )
}

async function ensureSession(channelId) {
  const existing = channelToSession.get(channelId)
  const existingSessionId = getStoredSessionId(existing)
  if (existingSessionId) {
    if (typeof existing !== "string") {
      setStoredSession(channelId, existingSessionId)
    } else {
      sessionToChannel.set(existingSessionId, channelId)
    }
    return { channelId, sessionId: existingSessionId }
  }

  const inflight = sessionCreateInflight.get(channelId)
  if (inflight) {
    return inflight
  }

  const creating = (async () => {
    const created = await withTimeout(
      (signal) =>
        opencode.client.session.create(
          {
            body: { title: `Discord channel ${channelId}` },
          },
          { signal },
        ),
      "discord session.create",
    )
    if (created.error) {
      throw new Error(created.error.message || "failed to create session")
    }

    const sessionId = created.data.id
    setStoredSession(channelId, sessionId)

    let url = null
    try {
      const share = await withTimeout(
        (signal) => opencode.client.session.share({ path: { id: sessionId } }, { signal }),
        "discord session.share",
      )
      url = share?.data?.share?.url || null
    } catch (err) {
      console.error("session share failed:", err?.message || err)
    }
    if (url) {
      await postChannelMessage(channelId, url)
    }

    return { channelId, sessionId }
  })()

  sessionCreateInflight.set(channelId, creating)
  try {
    return await creating
  } finally {
    if (sessionCreateInflight.get(channelId) === creating) {
      sessionCreateInflight.delete(channelId)
    }
  }
}

async function handleUserText(channelId, text) {
  const trimmed = String(text || "").trim()
  if (!trimmed) return

  if (trimmed === "/test") {
    await postChannelMessage(channelId, "Bot is working.")
    return
  }

  try {
    await limiter.run(async () => {
      let session
      try {
        session = await ensureSession(channelId)
      } catch {
        await postChannelMessage(channelId, "Sorry, I had trouble creating a session.")
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
          "discord session.prompt",
        )
      } catch {
        await postChannelMessage(channelId, "Sorry, I had trouble processing your message.")
        return
      }

      if (result.error) {
        await postChannelMessage(channelId, "Sorry, I had trouble processing your message.")
        return
      }

      const response = result.data
      const responseText = buildResponseText(response)

      await postChannelMessage(channelId, responseText)
    })
  } catch {
    await postChannelMessage(channelId, "Sorry, I had trouble processing your message.")
  }
}

function enqueueChannelMessage(channelId, text) {
  const pending = channelPendingCounts.get(channelId) || 0
  if (pending >= sessionPendingLimit) {
    const err = new Error(
      `session queue is full (channelId=${channelId}, maxPending=${sessionPendingLimit})`,
    )
    err.code = "SESSION_QUEUE_FULL"
    return Promise.reject(err)
  }

  channelPendingCounts.set(channelId, pending + 1)
  const prev = channelMessageInflight.get(channelId) || Promise.resolve()
  const next = prev
    .catch(() => {})
    .then(() => handleUserText(channelId, text))
    .catch((err) => {
      console.error("handle message failed:", err?.message || err)
    })
    .finally(() => {
      const remain = (channelPendingCounts.get(channelId) || 1) - 1
      if (remain <= 0) {
        channelPendingCounts.delete(channelId)
      } else {
        channelPendingCounts.set(channelId, remain)
      }
      if (channelMessageInflight.get(channelId) === next) {
        channelMessageInflight.delete(channelId)
      }
    })

  channelMessageInflight.set(channelId, next)
  return next
}

async function handleToolUpdate(part) {
  const update = getCompletedToolUpdate(part)
  if (!update) return

  const sessionId = update.sessionId
  const channelId = sessionToChannel.get(sessionId)
  if (!channelId) return

  const title = update.title
  const tool = update.tool
  await postChannelMessage(channelId, `*${tool}* - ${title}`)
}

void runEventSubscriptionLoop({
  label: "discord event subscription",
  subscribe: () =>
    withTimeout(
      (signal) => opencode.client.event.subscribe(undefined, { signal }),
      "discord event.subscribe",
    ),
  onEvent: async (event) => {
    if (event?.type !== "message.part.updated") return
    const part = event?.properties?.part
    await handleToolUpdate(part)
  },
})

client.on("messageCreate", (message) => {
  if (!message) return
  if (message.author?.bot) return
  if (!message.content) return

  const channelId = message.channelId
  queueMicrotask(() => {
    enqueueChannelMessage(channelId, message.content).catch((err) => {
      if (err?.code === "SESSION_QUEUE_FULL") {
        postChannelMessage(
          channelId,
          "Sorry, I'm handling too many messages in this channel. Please try again shortly.",
        ).catch(() => {})
        return
      }
      console.error("discord enqueue message failed:", err?.message || err)
    })
  })
})

client.once("ready", () => {
  console.log(`⚡️ Discord bot is running as ${client.user?.tag || "unknown"}`)
})

await client.login(discordToken)
