import assert from "node:assert/strict"
import fs from "node:fs/promises"
import { existsSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import { test } from "node:test"

import { createSessionStore } from "./session_store.mjs"

async function makeTempStorePath() {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "notify-kit-session-store-"))
  return {
    dir,
    file: path.join(dir, "sessions.json"),
  }
}

test("delete missing key does not create persistence file", async () => {
  const { dir, file } = await makeTempStorePath()
  const store = createSessionStore(file, { flushDebounceMs: 5 })
  await store.load()

  const deleted = store.delete("missing")
  assert.equal(deleted, false)

  await store.flush()
  await store.close()

  assert.equal(existsSync(file), false)
  await fs.rm(dir, { recursive: true, force: true })
})

test("delete existing key persists removal", async () => {
  const { dir, file } = await makeTempStorePath()
  const store = createSessionStore(file, { flushDebounceMs: 5 })
  await store.load()

  store.set("k", { sessionId: "s1" })
  await store.flush()
  assert.equal(existsSync(file), true)

  const deleted = store.delete("k")
  assert.equal(deleted, true)
  await store.flush()
  await store.close()

  const persisted = JSON.parse(await fs.readFile(file, "utf-8"))
  assert.deepEqual(persisted.entries, [])
  await fs.rm(dir, { recursive: true, force: true })
})

test("set returns evicted entries when maxEntries is reached", async () => {
  const { dir, file } = await makeTempStorePath()
  const store = createSessionStore(file, { maxEntries: 1, flushDebounceMs: 5 })
  await store.load()

  assert.deepEqual(store.set("a", "s1"), [])
  assert.deepEqual(store.set("b", "s2"), [["a", "s1"]])
  assert.deepEqual([...store.map.entries()], [["b", "s2"]])

  await store.close()
  await fs.rm(dir, { recursive: true, force: true })
})

test("load compacts oversized persisted entries with maxEntries", async () => {
  const { dir, file } = await makeTempStorePath()
  await fs.writeFile(
    file,
    `${JSON.stringify({ version: 1, entries: [["a", "s1"], ["b", "s2"], ["c", "s3"]] })}\n`,
    "utf-8",
  )

  const store = createSessionStore(file, { maxEntries: 2, flushDebounceMs: 5 })
  await store.load()
  assert.deepEqual([...store.map.entries()], [["b", "s2"], ["c", "s3"]])
  await store.close()

  const persisted = JSON.parse(await fs.readFile(file, "utf-8"))
  assert.deepEqual(persisted.entries, [["b", "s2"], ["c", "s3"]])
  await fs.rm(dir, { recursive: true, force: true })
})

test("load treats empty persisted file as no-op", async () => {
  const { dir, file } = await makeTempStorePath()
  await fs.writeFile(file, "   \n", "utf-8")

  const store = createSessionStore(file, { flushDebounceMs: 5 })

  const originalConsoleError = console.error
  const errors = []
  console.error = (...args) => {
    errors.push(args.map((v) => String(v)).join(" "))
  }

  try {
    await store.load()
    assert.deepEqual([...store.map.entries()], [])
    assert.equal(errors.some((line) => line.includes("session store parse failed")), false)
  } finally {
    console.error = originalConsoleError
    await store.close()
    await fs.rm(dir, { recursive: true, force: true })
  }
})

test("load migrates legacy object format to current entries format", async () => {
  const { dir, file } = await makeTempStorePath()
  await fs.writeFile(file, `${JSON.stringify({ a: "s1", b: "s2" })}\n`, "utf-8")

  const store = createSessionStore(file, { flushDebounceMs: 5 })
  await store.load()
  assert.deepEqual([...store.map.entries()], [["a", "s1"], ["b", "s2"]])
  await store.close()

  const persisted = JSON.parse(await fs.readFile(file, "utf-8"))
  assert.equal(persisted.version, 1)
  assert.deepEqual(persisted.entries, [["a", "s1"], ["b", "s2"]])
  await fs.rm(dir, { recursive: true, force: true })
})
