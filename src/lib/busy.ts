import { ref } from 'vue'

/** Long jobs in flight (Compare, Record, Disk Cleanup). The idle timer reads this before it closes the app. */
const active = ref(0)

/** Runs one long job and counts it as busy until it settles, even if it fails. */
export function trackJob<T>(work: () => Promise<T>): Promise<T> {
  active.value += 1
  return work().finally(() => {
    active.value -= 1
  })
}

export function jobsInFlight(): number {
  return active.value
}
