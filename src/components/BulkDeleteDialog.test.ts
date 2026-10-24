// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, h, ref } from 'vue'

const core = vi.hoisted(() => ({ invoke: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: core.invoke,
  isTauri: () => true,
  Channel: class {},
}))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))

import BulkDeleteDialog from './BulkDeleteDialog.vue'
import { appAdmin } from '@/lib/cleanup'

const preview = {
  items: [
    {
      id: 1,
      name: 'old',
      path: 'C:\old',
      size: 10,
      files: 1,
      isFolder: false,
      status: 'go',
      reason: null,
    },
  ],
  goCount: 1,
  goSize: 10,
  brake: null,
}

async function flush(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0))
}

function button(host: HTMLElement, label: string): HTMLButtonElement {
  const found = [...host.querySelectorAll('button')].find((item) =>
    item.textContent.includes(label),
  )
  if (!found) {
    throw new Error(`No button "${label}"`)
  }
  return found
}

describe('BulkDeleteDialog', () => {
  beforeEach(() => {
    HTMLDialogElement.prototype.showModal = vi.fn()
    HTMLDialogElement.prototype.close = vi.fn()
    appAdmin.elevated = true
  })

  afterEach(() => {
    core.invoke.mockReset()
    document.body.innerHTML = ''
  })

  it('asks again before a permanent delete and only then deletes', async () => {
    core.invoke.mockImplementation((command: string) => {
      if (command === 'cleanup_delete_preview') {
        return Promise.resolve(preview)
      }
      return Promise.resolve({ results: [], overview: null, logError: null })
    })
    const dialog = ref<{ show: (ids: number[]) => Promise<void> } | null>(null)
    const host = document.createElement('div')
    document.body.append(host)
    createApp({ render: () => h(BulkDeleteDialog, { ref: dialog }) }).mount(host)
    await dialog.value?.show([1])
    await flush()

    expect(host.textContent).toContain('moved to the Recycle Bin')
    button(host, 'Delete permanently').click()
    await flush()

    expect(host.textContent).toContain('deleted permanently. This cannot be undone.')
    expect(core.invoke).not.toHaveBeenCalledWith('cleanup_delete_many', expect.anything())

    button(host, 'Back').click()
    await flush()
    expect(host.textContent).toContain('moved to the Recycle Bin')
  })
})
