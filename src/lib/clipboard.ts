/** Copies text; falls back to a hidden textarea where the async clipboard API is refused. */
export async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text)
    return
  } catch {
    // Fall through to the legacy path.
  }
  const area = document.createElement('textarea')
  area.value = text
  area.setAttribute('readonly', '')
  area.style.position = 'fixed'
  area.style.opacity = '0'
  document.body.append(area)
  area.select()
  // eslint-disable-next-line @typescript-eslint/no-deprecated -- the only fallback left
  const copied = document.execCommand('copy')
  area.remove()
  if (!copied) {
    throw new Error('Could not copy to the clipboard.')
  }
}
