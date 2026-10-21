import { reactive, ref, watch } from 'vue'

/** Who the work is for. Shared by Compare, Record and Disk Cleanup; saved with each report. */
export interface JobInfo {
  client: string
  ticket: string
  technician: string
}

const jobKey = 'analyzer-job'
const brandKey = 'analyzer-brand'

export function loadJob(): JobInfo {
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(jobKey) ?? 'null')
    const fields = (saved && typeof saved === 'object' ? saved : {}) as Record<string, unknown>
    const text = (value: unknown): string => (typeof value === 'string' ? value : '')
    return {
      client: text(fields.client),
      ticket: text(fields.ticket),
      technician: text(fields.technician),
    }
  } catch {
    return { client: '', ticket: '', technician: '' }
  }
}

/** The job details last typed; kept between runs and app starts. */
export const job = reactive(loadJob())

watch(job, () => {
  try {
    localStorage.setItem(jobKey, JSON.stringify(job))
  } catch {
    // Not remembered; still used for this session.
  }
})

/** The job details to send with a run, trimmed. */
export function currentJob(): JobInfo {
  return {
    client: job.client.trim(),
    ticket: job.ticket.trim(),
    technician: job.technician.trim(),
  }
}

/** "Acme · T-1234": what a report row shows for its job, or '' when there is none. */
export function jobLabel(info: JobInfo): string {
  return [info.client, info.ticket]
    .map((part) => part.trim())
    .filter(Boolean)
    .join(' · ')
}

function readBrand(): string {
  try {
    return localStorage.getItem(brandKey) ?? ''
  } catch {
    return ''
  }
}

/** The company name at the top of exported reports (Settings). Empty: "Deep Server Analyzer". */
export const brand = ref(readBrand())

watch(brand, (value) => {
  try {
    localStorage.setItem(brandKey, value)
  } catch {
    // Not remembered.
  }
})
