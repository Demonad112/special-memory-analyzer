<script setup lang="ts">
import { Copy, Minus, Monitor, ShieldCheck, Square, X } from '@lucide/vue'
import { isTauri } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { onMounted, ref } from 'vue'
import logo from '@/assets/logo.png'
import { appInfo } from '@/lib/appInfo'
import { appAdmin, refreshElevated } from '@/lib/cleanup'

const maximized = ref(false)
const win = isTauri() ? getCurrentWindow() : null

onMounted(async () => {
  void refreshElevated()
  if (!win) {
    return
  }
  maximized.value = await win.isMaximized()
  await win.onResized(async () => {
    maximized.value = await win.isMaximized()
  })
})
</script>

<template>
  <header
    class="flex h-12 shrink-0 items-center"
    data-tauri-drag-region
  >
    <img
      :src="logo"
      alt=""
      class="pointer-events-none ml-4 size-4"
    />
    <span class="pointer-events-none ml-3 text-xs font-semibold tracking-wide"
      >Deep Server Analyzer</span
    >
    <span
      v-if="appInfo.machine"
      class="pointer-events-none ml-3 flex items-center gap-1.5 rounded-full bg-subtle px-2.5 py-0.5 text-xs text-muted"
      title="This computer"
    >
      <Monitor class="size-3" />
      {{ appInfo.machine }}
    </span>
    <span
      v-if="appAdmin.elevated"
      class="pointer-events-none ml-2 flex items-center gap-1.5 rounded-full bg-accent px-2.5 py-0.5 text-xs text-on-accent"
      title="Deep Server Analyzer is running as administrator"
    >
      <ShieldCheck class="size-3" />
      Administrator
    </span>
    <div
      class="flex-1 self-stretch"
      data-tauri-drag-region
    />
    <div class="flex self-stretch">
      <button
        class="grid w-[46px] place-items-center text-fg hover:bg-subtle"
        aria-label="Minimize"
        @click="win?.minimize()"
      >
        <Minus
          class="size-4"
          :stroke-width="1.25"
        />
      </button>
      <button
        class="grid w-[46px] place-items-center text-fg hover:bg-subtle"
        :aria-label="maximized ? 'Restore' : 'Maximize'"
        @click="win?.toggleMaximize()"
      >
        <component
          :is="maximized ? Copy : Square"
          class="size-3.5"
          :stroke-width="1.25"
        />
      </button>
      <button
        class="grid w-[46px] place-items-center text-fg hover:bg-close-hover hover:text-on-danger"
        aria-label="Close"
        @click="win?.close()"
      >
        <X
          class="size-4"
          :stroke-width="1.25"
        />
      </button>
    </div>
  </header>
</template>
