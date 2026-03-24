import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import path from "node:path"
import { test } from "node:test"
import { fileURLToPath, pathToFileURL } from "node:url"

import { buildResponseText, withTimeout } from "./opencode.mjs"

const here = path.dirname(fileURLToPath(import.meta.url))
const opencodeModuleUrl = pathToFileURL(path.join(here, "opencode.mjs")).href

function runNodeScript(script) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, ["--input-type=module", "-e", script], {
      stdio: ["ignore", "pipe", "pipe"],
    })

    let stdout = ""
    let stderr = ""
    child.stdout.on("data", (chunk) => {
      stdout += String(chunk)
    })
    child.stderr.on("data", (chunk) => {
      stderr += String(chunk)
    })
    child.on("error", reject)
    child.on("close", (code) => {
      resolve({ code, stdout, stderr })
    })
  })
}

test("runEventSubscriptionLoop retries when handler fails before concurrency cap", async () => {
  const script = `
import { runEventSubscriptionLoop } from ${JSON.stringify(opencodeModuleUrl)}

let subscribeCalls = 0
let first = true
console.error = () => {}

setTimeout(() => {
  console.log("SUBSCRIBE_CALLS=" + String(subscribeCalls))
  process.exit(0)
}, 400)

void runEventSubscriptionLoop({
  label: "test-loop",
  minBackoffMs: 10,
  maxBackoffMs: 10,
  jitterMs: 0,
  maxConcurrentOnEvent: 4,
  subscribe: async () => {
    subscribeCalls += 1
    return {
      stream: (async function* () {
        yield { id: 1 }
        if (first) {
          first = false
          await new Promise(() => {})
        }
      })(),
    }
  },
  onEvent: async () => {
    throw new Error("boom")
  },
})
`

  const { code, stdout, stderr } = await runNodeScript(script)
  assert.equal(code, 0, `child exited with non-zero code, stderr=${stderr}`)

  const match = stdout.match(/SUBSCRIBE_CALLS=(\d+)/)
  assert.ok(match, `missing subscribe count output, stdout=${stdout}`)
  const subscribeCalls = Number.parseInt(match[1], 10)
  assert.ok(Number.isFinite(subscribeCalls), `invalid subscribe count, stdout=${stdout}`)
  assert.ok(subscribeCalls >= 2, `expected loop retry, got subscribeCalls=${subscribeCalls}`)
})

test("runEventSubscriptionLoop retries when fast stream and fast handler failure race", async () => {
  const script = `
import { runEventSubscriptionLoop } from ${JSON.stringify(opencodeModuleUrl)}

let subscribeCalls = 0
console.error = () => {}

setTimeout(() => {
  console.log("SUBSCRIBE_CALLS=" + String(subscribeCalls))
  process.exit(0)
}, 450)

void runEventSubscriptionLoop({
  label: "test-loop-fast-fail",
  minBackoffMs: 10,
  maxBackoffMs: 10,
  jitterMs: 0,
  maxConcurrentOnEvent: 4,
  subscribe: async () => {
    subscribeCalls += 1
    return {
      stream: (async function* () {
        let i = 0
        while (i < 1000000) {
          yield { id: i }
          i += 1
        }
      })(),
    }
  },
  onEvent: async () => {
    throw new Error("boom")
  },
})
`

  const { code, stdout, stderr } = await runNodeScript(script)
  assert.equal(code, 0, `child exited with non-zero code, stderr=${stderr}`)

  const match = stdout.match(/SUBSCRIBE_CALLS=(\d+)/)
  assert.ok(match, `missing subscribe count output, stdout=${stdout}`)
  const subscribeCalls = Number.parseInt(match[1], 10)
  assert.ok(Number.isFinite(subscribeCalls), `invalid subscribe count, stdout=${stdout}`)
  assert.ok(subscribeCalls >= 2, `expected loop retry, got subscribeCalls=${subscribeCalls}`)
})

test("runEventSubscriptionLoop does not leak pending next() rejection on handler failure", async () => {
  const script = `
import { runEventSubscriptionLoop } from ${JSON.stringify(opencodeModuleUrl)}

let subscribeCalls = 0
let unhandled = 0
console.error = () => {}
process.on("unhandledRejection", () => {
  unhandled += 1
})

setTimeout(() => {
  console.log("SUBSCRIBE_CALLS=" + String(subscribeCalls))
  console.log("UNHANDLED=" + String(unhandled))
  process.exit(0)
}, 450)

void runEventSubscriptionLoop({
  label: "test-loop-pending-next",
  minBackoffMs: 10,
  maxBackoffMs: 10,
  jitterMs: 0,
  maxConcurrentOnEvent: 4,
  subscribe: async () => {
    subscribeCalls += 1
    let step = 0
    return {
      stream: {
        [Symbol.asyncIterator]() {
          return this
        },
        next() {
          step += 1
          if (step === 1) return Promise.resolve({ done: false, value: { id: 1 } })
          return new Promise((_, reject) => {
            setTimeout(() => reject(new Error("late-next-fail")), 30)
          })
        },
        return() {
          return Promise.resolve({ done: true, value: undefined })
        },
      },
    }
  },
  onEvent: async () => {
    throw new Error("boom")
  },
})
`

  const { code, stdout, stderr } = await runNodeScript(script)
  assert.equal(code, 0, `child exited with non-zero code, stderr=${stderr}`)

  const subscribeMatch = stdout.match(/SUBSCRIBE_CALLS=(\d+)/)
  assert.ok(subscribeMatch, `missing subscribe count output, stdout=${stdout}`)
  const subscribeCalls = Number.parseInt(subscribeMatch[1], 10)
  assert.ok(Number.isFinite(subscribeCalls), `invalid subscribe count, stdout=${stdout}`)
  assert.ok(subscribeCalls >= 2, `expected loop retry, got subscribeCalls=${subscribeCalls}`)

  const unhandledMatch = stdout.match(/UNHANDLED=(\d+)/)
  assert.ok(unhandledMatch, `missing unhandled count output, stdout=${stdout}`)
  const unhandled = Number.parseInt(unhandledMatch[1], 10)
  assert.ok(Number.isFinite(unhandled), `invalid unhandled count, stdout=${stdout}`)
  assert.equal(unhandled, 0, `expected no unhandled rejections, stdout=${stdout}`)
})

test("runEventSubscriptionLoop avoids Promise.race on inflight Set", async () => {
  const script = `
let setRaceCalls = 0
const originalRace = Promise.race.bind(Promise)
Promise.race = function (iterable) {
  if (iterable instanceof Set) {
    setRaceCalls += 1
  }
  return originalRace(iterable)
}

const { runEventSubscriptionLoop } = await import(${JSON.stringify(opencodeModuleUrl)})
console.error = () => {}

setTimeout(() => {
  console.log("SET_RACE_CALLS=" + String(setRaceCalls))
  process.exit(0)
}, 350)

void runEventSubscriptionLoop({
  label: "test-loop-no-set-race",
  minBackoffMs: 10,
  maxBackoffMs: 10,
  jitterMs: 0,
  maxConcurrentOnEvent: 2,
  subscribe: async () => {
    return {
      stream: (async function* () {
        for (let i = 0; i < 200; i += 1) {
          yield { id: i }
        }
        await new Promise(() => {})
      })(),
    }
  },
  onEvent: async () => {
    await new Promise((resolve) => setTimeout(resolve, 50))
  },
})
`

  const { code, stdout, stderr } = await runNodeScript(script)
  assert.equal(code, 0, `child exited with non-zero code, stderr=${stderr}`)

  const match = stdout.match(/SET_RACE_CALLS=(\d+)/)
  assert.ok(match, `missing Promise.race(Set) count output, stdout=${stdout}`)
  const setRaceCalls = Number.parseInt(match[1], 10)
  assert.ok(Number.isFinite(setRaceCalls), `invalid Promise.race(Set) count, stdout=${stdout}`)
  assert.equal(setRaceCalls, 0, `expected no Promise.race(Set), stdout=${stdout}`)
})

test("runEventSubscriptionLoop closes iterator when handler rejects with falsy value", async () => {
  const script = `
import { runEventSubscriptionLoop } from ${JSON.stringify(opencodeModuleUrl)}

let subscribeCalls = 0
let returnCalls = 0
console.error = () => {}

setTimeout(() => {
  console.log("SUBSCRIBE_CALLS=" + String(subscribeCalls))
  console.log("RETURN_CALLS=" + String(returnCalls))
  process.exit(0)
}, 450)

void runEventSubscriptionLoop({
  label: "test-loop-falsy-reject",
  minBackoffMs: 10,
  maxBackoffMs: 10,
  jitterMs: 0,
  maxConcurrentOnEvent: 2,
  subscribe: async () => {
    subscribeCalls += 1
    let step = 0
    return {
      stream: {
        [Symbol.asyncIterator]() {
          return this
        },
        next() {
          step += 1
          if (step === 1) return Promise.resolve({ done: false, value: { id: 1 } })
          return new Promise(() => {})
        },
        return() {
          returnCalls += 1
          return Promise.resolve({ done: true, value: undefined })
        },
      },
    }
  },
  onEvent: async () => Promise.reject(),
})
`

  const { code, stdout, stderr } = await runNodeScript(script)
  assert.equal(code, 0, `child exited with non-zero code, stderr=${stderr}`)

  const subscribeMatch = stdout.match(/SUBSCRIBE_CALLS=(\d+)/)
  assert.ok(subscribeMatch, `missing subscribe count output, stdout=${stdout}`)
  const subscribeCalls = Number.parseInt(subscribeMatch[1], 10)
  assert.ok(Number.isFinite(subscribeCalls), `invalid subscribe count, stdout=${stdout}`)
  assert.ok(subscribeCalls >= 2, `expected loop retry, got subscribeCalls=${subscribeCalls}`)

  const returnMatch = stdout.match(/RETURN_CALLS=(\d+)/)
  assert.ok(returnMatch, `missing return count output, stdout=${stdout}`)
  const returnCalls = Number.parseInt(returnMatch[1], 10)
  assert.ok(Number.isFinite(returnCalls), `invalid return count, stdout=${stdout}`)
  assert.ok(returnCalls >= 1, `expected iterator.return to be called, stdout=${stdout}`)
})

test("buildResponseText prefers info content", () => {
  const out = buildResponseText({
    info: { content: "from-info" },
    parts: [{ type: "text", text: "from-part" }],
  })
  assert.equal(out, "from-info")
})

test("buildResponseText joins text parts with newline", () => {
  const out = buildResponseText({
    parts: [
      { type: "tool", text: "ignored" },
      { type: "text", text: "line-1" },
      { type: "text", text: "line-2" },
    ],
  })
  assert.equal(out, "line-1\nline-2")
})

test("buildResponseText keeps join semantics for empty text segments", () => {
  const out = buildResponseText({
    parts: [
      { type: "text" },
      { type: "text", text: "line-2" },
    ],
  })
  assert.equal(out, "\nline-2")
})

test("buildResponseText falls back when no text is available", () => {
  const out = buildResponseText({
    parts: [{ type: "tool", text: "ignored" }],
  })
  assert.equal(out, "I received your message but didn't have a response.")
})

test("withTimeout handles synchronous task throws as rejected promises", async () => {
  await assert.rejects(
    () =>
      withTimeout(() => {
        throw new Error("sync-boom")
      }, "sync-task", 100),
    /sync-boom/,
  )
})
