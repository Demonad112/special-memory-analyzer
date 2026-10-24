import { computed, onBeforeUnmount, ref, watch, type ComputedRef } from 'vue'
import { formatClock } from './format'

/** "m:ss" since `startedAt` (epoch ms), ticking every 250 ms while `active()` is true. */
export function useElapsed(startedAt: () => number, active: () => boolean): ComputedRef<string> {
  const now = ref(Date.now())
  let ticker: ReturnType<typeof setInterval> | undefined
  watch(
    active,
    (on) => {
      clearInterval(ticker)
      if (on) {
        now.value = Date.now()
        ticker = setInterval(() => {
          now.value = Date.now()
        }, 250)
      }
    },
    { immediate: true },
  )
  onBeforeUnmount(() => {
    clearInterval(ticker)
  })
  return computed(() => formatClock(now.value - startedAt()))
}
