import { onBeforeUnmount, ref, type Ref } from 'vue'

/** How long a toast stays up; every page uses the same time. */
export const TOAST_MS = 4000

/** A short confirmation or error at the bottom of the window, cleared on its own. */
export function useToast(): { toast: Ref<string>; showToast: (message: string) => void } {
  const toast = ref('')
  let timer: ReturnType<typeof setTimeout> | undefined
  function showToast(message: string): void {
    toast.value = message
    clearTimeout(timer)
    timer = setTimeout(() => {
      toast.value = ''
    }, TOAST_MS)
  }
  onBeforeUnmount(() => {
    clearTimeout(timer)
  })
  return { toast, showToast }
}
