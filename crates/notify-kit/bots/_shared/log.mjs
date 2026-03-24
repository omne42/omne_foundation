import process from "node:process"

export function isVerbose() {
  const v = String(process.env.OPENCODE_BOT_VERBOSE || "").trim()
  if (v === "1" || v.toLowerCase() === "true") return true
  return Boolean(process.env.DEBUG && String(process.env.DEBUG).trim() !== "")
}

export function logError(context, err) {
  if (!isVerbose()) return
  const stack = err?.stack
  if (typeof stack === "string" && stack.trim() !== "") {
    console.error(context, stack)
    return
  }

  const msg = err?.message || String(err)
  console.error(context, msg)
}

export function ignoreError(promise, context) {
  return Promise.resolve(promise).catch((err) => {
    logError(context, err)
  })
}
