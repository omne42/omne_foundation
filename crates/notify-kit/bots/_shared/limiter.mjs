export class LimiterQueueFullError extends Error {
  constructor(maxQueue) {
    super(`limiter queue is full (maxQueue=${maxQueue})`)
    this.name = "LimiterQueueFullError"
    this.code = "LIMITER_QUEUE_FULL"
  }
}

const DEFAULT_MAX_QUEUE = 2048

export function createLimiter({ maxInflight = 4, maxQueue = DEFAULT_MAX_QUEUE } = {}) {
  const limit = Number.parseInt(String(maxInflight), 10)
  const concurrency = Number.isFinite(limit) && limit > 0 ? limit : 4
  const queueLimitValue = Number.parseInt(String(maxQueue), 10)
  const queueLimit =
    Number.isFinite(queueLimitValue) && queueLimitValue > 0 ? queueLimitValue : DEFAULT_MAX_QUEUE

  let inflight = 0
  let queue = []
  let head = 0

  const compactQueue = () => {
    if (head === 0) return
    if (head === queue.length || head >= 1024 || head * 2 >= queue.length) {
      queue = queue.slice(head)
      head = 0
    }
  }

  const pump = () => {
    while (inflight < concurrency && head < queue.length) {
      const idx = head
      const item = queue[idx]
      queue[idx] = undefined
      head += 1
      if (!item) continue
      inflight += 1
      Promise.resolve()
        .then(item.fn)
        .then(item.resolve, item.reject)
        .finally(() => {
          inflight -= 1
          compactQueue()
          pump()
        })
    }
    compactQueue()
  }

  const run = (fn) =>
    new Promise((resolve, reject) => {
      const queued = queue.length - head
      if (queueLimit > 0 && queued >= queueLimit) {
        reject(new LimiterQueueFullError(queueLimit))
        return
      }
      queue.push({ fn, resolve, reject })
      pump()
    })

  return { run, maxInflight: concurrency, maxQueue: queueLimit }
}
