import { invoke, isTauri } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { ref } from 'vue'

export interface AppInfo {
  version: string
  machine: string
  mica: boolean
}

export const appInfo = ref<AppInfo>({ version: 'dev', machine: '', mica: false })

/** Loads app info, switches on Mica, then shows the window (created hidden to avoid a flash). */
export async function startShell(): Promise<void> {
  if (!isTauri()) {
    return
  }
  try {
    appInfo.value = await invoke<AppInfo>('app_info')
    document.documentElement.classList.toggle('mica', appInfo.value.mica)
  } finally {
    await getCurrentWindow().show()
  }
}
