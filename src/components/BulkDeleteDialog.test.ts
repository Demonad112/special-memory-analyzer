// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
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

  /** Mounts the dialog, opens it on item 1 and waits for the dry run. */
  async function open(dryRun: object = preview) {
    const wrapper = mount(BulkDeleteDialog, { attachTo: document.body })
    const pending: { release: () => void } = { release: () => undefined }
    core.invoke.mockImplementation((command: string) => {
      if (command === 'cleanup_delete_preview') {
        return Promise.resolve(dryRun)
      }
      return new Promise((resolve) => {
        pending.release = () => {
          resolve({ results: [], overview: { warning: null }, logError: null })
        }
      })
    })
    await (wrapper.vm as unknown as { show: (ids: number[]) => Promise<void> }).show([1])
    await flush()
    return { wrapper, pending }
  }

  function deleteCall(): unknown[] | undefined {
    return core.invoke.mock.calls.find((call) => call[0] === 'cleanup_delete_many')
  }

  it('sends a permanent delete only after the second confirmation, with no token', async () => {
    const { wrapper, pending } = await open()
    await wrapper
      .findAll('button')
      .find((b) => b.text().includes('Delete permanently'))
      ?.trigger('click')
    await wrapper
      .findAll('button')
      .find((b) => b.text().includes('permanently') && b.text().startsWith('Yes'))
      ?.trigger('click')
    await flush()

    expect(deleteCall()?.[1]).toEqual({ ids: [1], permanent: true, confirmToken: undefined })
    pending.release()
    wrapper.unmount()
  })

  it('blocks Escape while deleting and allows it afterwards', async () => {
    const { wrapper, pending } = await open()
    await wrapper
      .findAll('button')
      .find((b) => b.text().includes('Move to Recycle Bin'))
      ?.trigger('click')
    await flush()
    expect(wrapper.text()).toContain('Deleting…')

    const dialog = wrapper.find('dialog').element
    const while_running = new Event('cancel', { cancelable: true })
    dialog.dispatchEvent(while_running)
    expect(while_running.defaultPrevented).toBe(true)

    pending.release()
    await flush()
    expect(wrapper.text()).toContain('Done')
    const after = new Event('cancel', { cancelable: true })
    dialog.dispatchEvent(after)
    expect(after.defaultPrevented).toBe(false)
    wrapper.unmount()
  })

  it('passes the big-cleanup token through to the delete', async () => {
    const { wrapper, pending } = await open({
      ...preview,
      brake: { token: 'tok-1', biggest: preview.items },
    })
    await wrapper
      .findAll('button')
      .find((b) => b.text().includes('Move to Recycle Bin'))
      ?.trigger('click')
    expect(wrapper.text()).toContain('This is a big cleanup')
    expect(deleteCall()).toBeUndefined()

    await wrapper
      .findAll('button')
      .find((b) => b.text().startsWith('Yes, recycle'))
      ?.trigger('click')
    await flush()
    expect(deleteCall()?.[1]).toEqual({ ids: [1], permanent: false, confirmToken: 'tok-1' })
    pending.release()
    wrapper.unmount()
  })
})
