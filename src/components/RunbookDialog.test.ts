// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'

const core = vi.hoisted(() => ({ invoke: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: core.invoke, isTauri: () => true }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ save: vi.fn(), open: vi.fn() }))

import RunbookDialog from './RunbookDialog.vue'
import type { ReportEntry } from '@/lib/reports'

function row(id: string, kind: ReportEntry['kind'], ticket: string): ReportEntry {
  return {
    id,
    kind,
    createdAtMs: 1,
    title: id,
    subject: 'S',
    headline: 'h',
    problem: false,
    job: { client: 'Acme', ticket, technician: 'Sam' },
    csvLayouts: [],
  }
}

const entries = [
  row('other', 'compare', 'T-9'),
  row('copy', 'record', 'T-1'),
  row('ready', 'readiness', 'T-1'),
  row('usage', 'diskUsage', 'T-1'),
]

beforeEach(() => {
  vi.clearAllMocks()
  HTMLDialogElement.prototype.showModal = vi.fn()
  HTMLDialogElement.prototype.close = vi.fn()
})

function opened() {
  const wrapper = mount(RunbookDialog, { props: { entries } })
  ;(wrapper.vm as unknown as { open: () => void }).open()
  return wrapper
}

describe('RunbookDialog', () => {
  it('lists only the kinds a runbook can hold and disables Make until something is picked', async () => {
    const wrapper = opened()
    await flushPromises()
    expect(wrapper.findAll('li input[type="checkbox"]')).toHaveLength(3)
    expect(wrapper.text()).not.toContain('usage')
    expect(wrapper.find('button.bg-accent').attributes('disabled')).toBeDefined()
  })

  it('suggests the first pick’s ticket and sends the picks as ids', async () => {
    const wrapper = opened()
    await wrapper.findAll('li input[type="checkbox"]')[1]?.setValue(true)
    // The first pick was the copy run: the readiness report with the same ticket is suggested.
    expect(wrapper.text()).toContain('Same ticket')
    const ticket = wrapper.findAll('input[type="text"]')[1]
    expect((ticket?.element as HTMLInputElement).value).toBe('T-1')

    core.invoke.mockResolvedValueOnce(row('runbook-1', 'runbook', 'T-1'))
    await wrapper.find('button.bg-accent').trigger('click')
    await flushPromises()
    expect(core.invoke).toHaveBeenCalledWith('runbook_create', {
      job: { client: 'Acme', ticket: 'T-1', technician: 'Sam' },
      parts: [{ kind: 'record', id: 'copy' }],
    })
    expect(wrapper.emitted('created')?.[0]?.[0]).toMatchObject({ id: 'runbook-1' })
  })

  it('shows the error from Rust and keeps the dialog open', async () => {
    const wrapper = opened()
    await wrapper.findAll('li input[type="checkbox"]')[0]?.setValue(true)
    core.invoke.mockRejectedValueOnce('A report picked for the runbook is no longer there (x).')
    await wrapper.find('button.bg-accent').trigger('click')
    await flushPromises()
    expect(wrapper.find('[role="alert"]').text()).toContain('no longer there')
    expect(wrapper.emitted('created')).toBeUndefined()
  })
})
