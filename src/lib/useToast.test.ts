// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, type Ref } from 'vue'
import { TOAST_MS, useToast } from './useToast'

describe('useToast', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  it('shows a message and clears it after TOAST_MS, restarting on a new one', () => {
    vi.useFakeTimers()
    let toast!: Ref<string>
    let showToast!: (message: string) => void
    const app = createApp(
      defineComponent({
        setup() {
          ;({ toast, showToast } = useToast())
          return () => h('div')
        },
      }),
    )
    app.mount(document.createElement('div'))

    showToast('one')
    vi.advanceTimersByTime(TOAST_MS - 1)
    expect(toast.value).toBe('one')
    showToast('two')
    vi.advanceTimersByTime(TOAST_MS - 1)
    expect(toast.value).toBe('two')
    vi.advanceTimersByTime(1)
    expect(toast.value).toBe('')
    app.unmount()
  })
})
