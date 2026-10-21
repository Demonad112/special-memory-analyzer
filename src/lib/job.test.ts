import { beforeEach, describe, expect, it } from 'vitest'
import { nextTick } from 'vue'
import { brand, currentJob, job, jobLabel, loadJob } from './job'

beforeEach(() => {
  localStorage.clear()
})

describe('job details', () => {
  it('reads saved details and ignores anything malformed', () => {
    localStorage.setItem(
      'analyzer-job',
      JSON.stringify({ client: 'Acme', ticket: 42, technician: 'Sam' }),
    )
    expect(loadJob()).toEqual({ client: 'Acme', ticket: '', technician: 'Sam' })
    localStorage.setItem('analyzer-job', '{oops')
    expect(loadJob()).toEqual({ client: '', ticket: '', technician: '' })
  })

  it('remembers what was typed and sends it trimmed', async () => {
    job.client = '  Acme  '
    job.ticket = 'T-1'
    await nextTick()
    expect(JSON.parse(localStorage.getItem('analyzer-job') ?? '{}')).toMatchObject({
      client: '  Acme  ',
    })
    expect(currentJob()).toMatchObject({ client: 'Acme', ticket: 'T-1' })
  })

  it('labels a job by client and ticket', () => {
    expect(jobLabel({ client: 'Acme', ticket: 'T-1', technician: 'Sam' })).toBe('Acme · T-1')
    expect(jobLabel({ client: '', ticket: ' T-1 ', technician: '' })).toBe('T-1')
    expect(jobLabel({ client: '', ticket: '', technician: 'Sam' })).toBe('')
  })

  it('remembers the company name', async () => {
    brand.value = 'Acme IT'
    await nextTick()
    expect(localStorage.getItem('analyzer-brand')).toBe('Acme IT')
  })
})
