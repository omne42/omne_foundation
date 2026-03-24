import process from "node:process"

export function assertEnv(name, { optional = false } = {}) {
  const value = process.env[name]
  if ((value === undefined || String(value).trim() === "") && !optional) {
    throw new Error(`missing required env: ${name}`)
  }
  return value
}

export function buildResponseText(response) {
  if (response?.info?.content) {
    return response.info.content
  }

  const parts = response?.parts
  if (Array.isArray(parts)) {
    const textParts = []
    for (const part of parts) {
      if (part?.type !== "text") continue
      textParts.push(part.text !== undefined && part.text !== null ? String(part.text) : "")
    }
    const joined = textParts.join("\n")
    if (joined) return joined
  }

  return "I received your message but didn't have a response."
}

export function getCompletedToolUpdate(part) {
  if (!part || part.type !== "tool") return null
  if (!part.state || part.state.status !== "completed") return null
  return {
    sessionId: part.sessionID,
    title: part.state.title || "completed",
    tool: part.tool || "tool",
  }
}

export function getOpencodeTimeoutMs() {
  const value = Number.parseInt(String(process.env.OPENCODE_BOT_OPENCODE_TIMEOUT_MS || "45000"), 10)
  return Number.isFinite(value) && value > 0 ? value : 45000
}

function getOpencodeAbortDrainMs() {
  const value = Number.parseInt(String(process.env.OPENCODE_BOT_OPENCODE_ABORT_DRAIN_MS || "3000"), 10)
  return Number.isFinite(value) && value > 0 ? value : 3000
}

async function waitForAbortDrain(taskPromise, timeoutMs) {
  if (!taskPromise || typeof taskPromise.then !== "function") return
  let timer = null
  await Promise.race([
    taskPromise.catch(() => {}).finally(() => {
      if (timer) clearTimeout(timer)
    }),
    new Promise((resolve) => {
      timer = setTimeout(resolve, timeoutMs)
      if (typeof timer?.unref === "function") {
        timer.unref()
      }
    }),
  ])
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

function getEventHandlerTimeoutMs() {
  const value = Number.parseInt(String(process.env.OPENCODE_BOT_EVENT_HANDLER_TIMEOUT_MS || "15000"), 10)
  return Number.isFinite(value) && value > 0 ? value : 15000
}

function getEventDrainTimeoutMs() {
  const value = Number.parseInt(String(process.env.OPENCODE_BOT_EVENT_DRAIN_TIMEOUT_MS || "3000"), 10)
  return Number.isFinite(value) && value > 0 ? value : 3000
}

async function settleInflight(inflight, timeoutMs) {
  if (!inflight || inflight.size === 0) return true
  const waitAll = Promise.allSettled([...inflight]).then(() => true)
  let timer = null
  const waitTimeout = new Promise((resolve) => {
    timer = setTimeout(() => resolve(false), timeoutMs)
    if (typeof timer?.unref === "function") {
      timer.unref()
    }
  })
  try {
    return await Promise.race([waitAll, waitTimeout])
  } finally {
    if (timer) clearTimeout(timer)
  }
}

function getEventHandlerConcurrency() {
  const value = Number.parseInt(String(process.env.OPENCODE_BOT_EVENT_HANDLER_CONCURRENCY || "4"), 10)
  return Number.isFinite(value) && value > 0 ? value : 4
}

function normalizeLoopError(err, fallbackMessage) {
  if (err instanceof Error) return err
  if (typeof err === "string" && err.trim() !== "") return new Error(err)
  if (err === null || err === undefined) return new Error(fallbackMessage)
  return new Error(String(err))
}

export async function withTimeout(taskOrPromise, label, timeoutMs = getOpencodeTimeoutMs()) {
  const supportsAbort = typeof taskOrPromise === "function"
  const controller =
    supportsAbort && typeof AbortController !== "undefined" ? new AbortController() : null
  const taskPromise = supportsAbort
    ? Promise.resolve().then(() => taskOrPromise(controller?.signal))
    : Promise.resolve(taskOrPromise)

  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
    return taskPromise
  }

  const timeoutMsg = `${label} timed out after ${timeoutMs}ms`
  let timer = null
  let timedOut = false
  const timeoutPromise = new Promise((_, reject) => {
    timer = setTimeout(() => {
      timedOut = true
      if (controller) {
        try {
          controller.abort(new Error(timeoutMsg))
        } catch {
          controller.abort()
        }
      }
      reject(new Error(timeoutMsg))
    }, timeoutMs)
    if (typeof timer?.unref === "function") {
      timer.unref()
    }
  })

  try {
    return await Promise.race([taskPromise, timeoutPromise])
  } catch (err) {
    if (timedOut && supportsAbort) {
      await waitForAbortDrain(taskPromise, getOpencodeAbortDrainMs())
    }
    if (timedOut && !supportsAbort && taskPromise && typeof taskPromise.catch === "function") {
      // Prevent late rejections from becoming unhandled when timeout wins Promise.race.
      taskPromise.catch(() => {})
    }
    throw err
  } finally {
    if (timer) clearTimeout(timer)
  }
}

export async function runEventSubscriptionLoop({
  label,
  subscribe,
  onEvent,
  minBackoffMs = 1000,
  maxBackoffMs = 30000,
  jitterMs = 500,
  maxConcurrentOnEvent = getEventHandlerConcurrency(),
}) {
  const maxConcurrent = Number.isFinite(maxConcurrentOnEvent) && maxConcurrentOnEvent > 0
    ? Math.trunc(maxConcurrentOnEvent)
    : 1
  const eventTimeoutMs = getEventHandlerTimeoutMs()
  const drainTimeoutMs = getEventDrainTimeoutMs()
  let retries = 0
  for (;;) {
    try {
      const events = await subscribe()
      retries = 0
      const stream = events?.stream
      if (!stream || typeof stream[Symbol.asyncIterator] !== "function") {
        throw new Error(`${label} stream is not async iterable`)
      }
      const iterator = stream[Symbol.asyncIterator]()
      const inflight = new Set()
      const settledResults = []
      let settledHead = 0
      let settledSignal = null

      const compactSettledResults = () => {
        if (settledHead === 0) return
        if (
          settledHead === settledResults.length ||
          settledHead >= 1024 ||
          settledHead * 2 >= settledResults.length
        ) {
          settledResults.splice(0, settledHead)
          settledHead = 0
        }
      }

      const hasSettledResults = () => settledHead < settledResults.length

      const dequeueSettledResult = () => {
        if (!hasSettledResults()) return null
        const result = settledResults[settledHead]
        settledResults[settledHead] = undefined
        settledHead += 1
        compactSettledResults()
        return result || null
      }

      const waitForSettledSignal = () => {
        if (settledSignal) return settledSignal.promise
        let resolve
        const promise = new Promise((r) => {
          resolve = r
        })
        settledSignal = { promise, resolve }
        return promise
      }

      const notifySettledResult = () => {
        if (!settledSignal) return
        settledSignal.resolve()
        settledSignal = null
      }

      const addInflightTask = (event) => {
        const task = withTimeout(
          (signal) => Promise.resolve().then(() => onEvent(event, signal)),
          `${label} onEvent`,
          eventTimeoutMs,
        )
        const tracked = task
          .then(() => null)
          .catch((error) => ({
            error: normalizeLoopError(error, `${label} onEvent failed`),
          }))
        inflight.add(tracked)
        tracked.then((result) => {
          inflight.delete(tracked)
          settledResults.push(result)
          notifySettledResult()
        })
      }

      const waitForNextSettledResult = async () => {
        while (!hasSettledResults()) {
          if (inflight.size === 0) return null
          await waitForSettledSignal()
        }
        return dequeueSettledResult()
      }

      let loopError = null
      let streamEnded = false
      let pendingNext = iterator.next()

      while (!streamEnded || inflight.size > 0) {
        if (hasSettledResults()) {
          const result = dequeueSettledResult()
          if (result?.error) {
            loopError = result.error
            break
          }
          continue
        }

        if (inflight.size >= maxConcurrent || streamEnded) {
          if (inflight.size === 0) break
          const result = await waitForNextSettledResult()
          if (result?.error) {
            loopError = result.error
            break
          }
          continue
        }

        if (inflight.size === 0) {
          const step = await pendingNext
          if (step.done) {
            streamEnded = true
            continue
          }
          pendingNext = iterator.next()
          addInflightTask(step.value)
          continue
        }

        const outcome = await Promise.race([
          pendingNext.then(
            (step) => ({ kind: "event", step }),
            (error) => ({ kind: "event_error", error }),
          ),
          waitForSettledSignal().then(() => ({ kind: "result_ready" })),
        ])

        if (outcome.kind === "event_error") {
          loopError = normalizeLoopError(outcome.error, `${label} stream read failed`)
          break
        }

        if (outcome.kind === "result_ready") {
          const result = dequeueSettledResult()
          if (result?.error) {
            loopError = result.error
            break
          }
          continue
        }

        if (outcome.step.done) {
          streamEnded = true
          continue
        }

        pendingNext = iterator.next()
        addInflightTask(outcome.step.value)
      }

      if (loopError && pendingNext && typeof pendingNext.catch === "function") {
        // If the loop exits due to a handler failure, pending `next()` may reject later.
        // Consume it to avoid late unhandled rejection noise on retry.
        pendingNext.catch(() => {})
      }
      if (loopError && typeof iterator.return === "function") {
        Promise.resolve()
          .then(() => iterator.return())
          .catch(() => {})
      }
      const settled = await settleInflight(inflight, drainTimeoutMs)
      if (!settled) {
        console.error(`${label} drain timed out after ${drainTimeoutMs}ms; retrying`)
      }
      if (loopError) throw loopError
      throw new Error(`${label} stream ended`)
    } catch (err) {
      retries += 1
      const exp = Math.min(retries - 1, 6)
      const baseDelay = Math.min(maxBackoffMs, minBackoffMs * 2 ** exp)
      const jitterRange = Math.max(0, Number.parseInt(String(jitterMs), 10))
      const jitter = jitterRange > 0 ? Math.floor(Math.random() * (jitterRange + 1)) : 0
      const delayMs = baseDelay + jitter
      console.error(`${label} failed: ${err?.message || err}; retrying in ${delayMs}ms`)
      await sleep(delayMs)
    }
  }
}
