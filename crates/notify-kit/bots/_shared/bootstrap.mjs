import process from "node:process"

import { createLimiter } from "./limiter.mjs"
import { createSessionStore } from "./session_store.mjs"

export function createBotLimiter() {
  return createLimiter({
    maxInflight: process.env.OPENCODE_BOT_MAX_INFLIGHT || "4",
    maxQueue: process.env.OPENCODE_BOT_MAX_QUEUE || "2048",
  })
}

export async function createBotSessionStore() {
  const flushDebounceMsValue = Number.parseInt(
    String(process.env.OPENCODE_SESSION_STORE_FLUSH_DEBOUNCE_MS || "250"),
    10,
  )
  const flushDebounceMs =
    Number.isFinite(flushDebounceMsValue) && flushDebounceMsValue > 0 ? flushDebounceMsValue : 250
  const maxFileBytesValue = Number.parseInt(
    String(process.env.OPENCODE_SESSION_STORE_MAX_FILE_BYTES || `${20 * 1024 * 1024}`),
    10,
  )
  const maxFileBytes =
    Number.isFinite(maxFileBytesValue) && maxFileBytesValue > 0 ? maxFileBytesValue : 20 * 1024 * 1024

  const store = createSessionStore(process.env.OPENCODE_SESSION_STORE_PATH, {
    rootDir: process.env.OPENCODE_SESSION_STORE_ROOT || process.cwd(),
    maxEntries: process.env.OPENCODE_SESSION_STORE_MAX_ENTRIES || "20000",
    flushDebounceMs,
    maxFileBytes,
  })
  await store.load()
  store.installExitHooks()
  if (store.enabled) {
    console.log(`🗄️ Session store enabled: ${store.path}`)
  }
  return store
}
