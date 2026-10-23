// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createIdleWatch, loadIdleMinutes } from './idleTimeout'
import { trackJob, jobsInFlight } from './busy'

const minute = 60_000

function fakeClock() {
  let now = 0
  return {
    now: () => now,
    advance: (ms: number) => {
      now += ms
    },
  }
}

function setup(timeoutMinutes = 30) {
  const clock = fakeClock()
  const state = { busy: false, warnings: [] as (number | null)[], expired: 0 }
  const watch = createIdleWatch({
    timeoutMs: () => timeoutMinutes * minute,
    now: clock.now,
    busy: () => state.busy,
    onWarn: (remaining) => state.warnings.push(remaining),
    onExpire: () => {
      state.expired += 1
    },
  })
  // Runs the timer for `ms`, once per second, the way the app does.
  const run = (ms: number) => {
    for (let elapsed = 0; elapsed < ms; elapsed += 1000) {
      clock.advance(1000)
      watch.tick()
    }
  }
  return { clock, state, watch, run }
}

describe('idle watch', () => {
  it('warns one minute before the timeout and closes at the timeout', () => {
    const { state, run } = setup(30)
    run(29 * minute - 1000)
    expect(state.warnings).toEqual([])
    run(1000)
    expect(state.warnings.length).toBeGreaterThan(0)
    expect(state.expired).toBe(0)
    run(minute)
    expect(state.expired).toBe(1)
  })

  it('any activity cancels the warning and restarts the clock', () => {
    const { state, watch, run } = setup(30)
    run(30 * minute - 30_000)
    expect(state.warnings.at(-1)).not.toBeNull()
    watch.activity()
    expect(state.warnings.at(-1)).toBeNull()
    run(29 * minute)
    expect(state.expired).toBe(0)
  })

  it('never closes while a job is busy, and counts idle from the end of the job', () => {
    const { state, run } = setup(30)
    state.busy = true
    run(45 * minute)
    expect(state.expired).toBe(0)
    state.busy = false
    run(29 * minute)
    expect(state.expired).toBe(0)
    run(minute + 1000)
    expect(state.expired).toBe(1)
  })

  it('hides the warning when a job starts', () => {
    const { state, run } = setup(30)
    run(30 * minute - 30_000)
    state.busy = true
    run(1000)
    expect(state.warnings.at(-1)).toBeNull()
    expect(state.expired).toBe(0)
  })

  it('never warns or closes when the timer is off', () => {
    const { state, run } = setup(0)
    run(600 * minute)
    expect(state.warnings).toEqual([])
    expect(state.expired).toBe(0)
  })
})

describe('idle minutes setting', () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it('defaults to 30 minutes when nothing is saved', () => {
    expect(loadIdleMinutes()).toBe(30)
  })

  it('keeps a saved 0 as off, and rejects values that are not a choice', () => {
    localStorage.setItem('analyzer-idle-timeout', '0')
    expect(loadIdleMinutes()).toBe(0)
    localStorage.setItem('analyzer-idle-timeout', '7')
    expect(loadIdleMinutes()).toBe(30)
    localStorage.setItem('analyzer-idle-timeout', 'soon')
    expect(loadIdleMinutes()).toBe(30)
  })
})

describe('trackJob', () => {
  it('counts a job as busy until it settles, including when it fails', async () => {
    const work = vi.fn(() => Promise.reject(new Error('boom')))
    const before = jobsInFlight()
    await expect(trackJob(work)).rejects.toThrow('boom')
    expect(jobsInFlight()).toBe(before)
  })

  it('is busy while the job is pending', async () => {
    let finish: (value: number) => void = () => undefined
    const pending = trackJob(() => new Promise<number>((resolve) => (finish = resolve)))
    expect(jobsInFlight()).toBeGreaterThan(0)
    finish(1)
    await pending
  })
})
