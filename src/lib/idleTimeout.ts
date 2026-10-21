import { invoke, isTauri } from '@tauri-apps/api/core'
import { ref } from 'vue'
import { cleanupJob } from './cleanup'
import { jobsInFlight } from './busy'
import { recordJob } from './record'

/** Minutes of inactivity before the app closes. 0 turns the timer off. */
export const idleChoices = [0, 15, 30, 60, 120]

const storageKey = 'analyzer-idle-timeout'
const defaultMinutes = 30
const warningMs = 60_000
const activityThrottleMs = 1_000
const tickMs = 1_000
const activityEvents = ['pointermove', 'pointerdown', 'keydown', 'wheel', 'touchstart', 'focus']

export function loadIdleMinutes(): number {
  try {
    const saved = localStorage.getItem(storageKey)
    if (saved === null) return defaultMinutes
    const minutes = Number(saved)
    return idleChoices.includes(minutes) ? minutes : defaultMinutes
  } catch {
    return defaultMinutes
  }
}

export const idleMinutes = ref(loadIdleMinutes())

export function setIdleMinutes(minutes: number): void {
  idleMinutes.value = minutes
  try {
    localStorage.setItem(storageKey, String(minutes))
  } catch {
    // Storage is blocked; the choice still applies until the app closes.
  }
}

/** Milliseconds left before the close, while the warning is showing; null when it is not. */
export const idleWarningMs = ref<number | null>(null)

export interface IdleWatchOptions {
  timeoutMs: () => number
  now: () => number
  busy: () => boolean
  onWarn: (remainingMs: number | null) => void
  onExpire: () => void
}

export interface IdleWatch {
  activity: () => void
  tick: () => void
}

/** The idle rules, with the clock and the job check passed in so they can be tested without timers. */
export function createIdleWatch(options: IdleWatchOptions): IdleWatch {
  let lastActivity = options.now()
  let lastRecorded = lastActivity
  let warning = false
  let expired = false

  const hideWarning = (): void => {
    if (warning) {
      warning = false
      options.onWarn(null)
    }
  }

  return {
    activity() {
      // Any activity cancels the warning straight away; the clock is throttled for the common case.
      if (warning) {
        hideWarning()
        lastActivity = options.now()
        lastRecorded = lastActivity
        return
      }
      const at = options.now()
      if (at - lastRecorded < activityThrottleMs) return
      lastRecorded = at
      lastActivity = at
    },
    tick() {
      const timeout = options.timeoutMs()
      if (timeout <= 0) {
        hideWarning()
        return
      }
      const at = options.now()
      if (options.busy()) {
        // A job is running or its state is unreadable: never close, and count idle from its end.
        hideWarning()
        lastActivity = at
        lastRecorded = at
        return
      }
      const idle = at - lastActivity
      if (expired) return
      if (idle >= timeout) {
        expired = true
        options.onExpire()
        return
      }
      if (idle >= timeout - warningMs) {
        warning = true
        options.onWarn(timeout - idle)
      }
    },
  }
}

/** A job is busy when a tracked job is in flight or a Record or Cleanup run is marked running. */
function jobBusy(): boolean {
  try {
    return jobsInFlight() > 0 || recordJob.running || cleanupJob.running
  } catch {
    return true
  }
}

async function closeApp(): Promise<void> {
  if (isTauri()) {
    await invoke('app_exit')
  } else {
    window.close()
  }
}

let activeWatch: IdleWatch | null = null

/** Called by the warning's "Stay open" button. */
export function stayOpen(): void {
  activeWatch?.activity()
}

/** Starts the idle timer and returns a function that stops it. */
export function startIdleTimeout(): () => void {
  const watch = createIdleWatch({
    timeoutMs: () => idleMinutes.value * 60_000,
    now: () => Date.now(),
    busy: jobBusy,
    onWarn: (remaining) => {
      idleWarningMs.value = remaining
    },
    onExpire: () => {
      void closeApp()
    },
  })
  activeWatch = watch
  const onActivity = (): void => watch.activity()
  for (const name of activityEvents) {
    window.addEventListener(name, onActivity, { passive: true })
  }
  const timer = window.setInterval(() => watch.tick(), tickMs)
  return () => {
    window.clearInterval(timer)
    for (const name of activityEvents) {
      window.removeEventListener(name, onActivity)
    }
    activeWatch = null
    idleWarningMs.value = null
  }
}
