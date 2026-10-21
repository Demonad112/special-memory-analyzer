<script setup lang="ts">
import { HardDrive, Usb } from '@lucide/vue'
import type { Drive } from '@/lib/cleanup'
import { formatBytes } from '@/lib/format'

defineProps<{ drives: Drive[]; disabled?: boolean }>()
const emit = defineEmits<{ pick: [drive: Drive] }>()

function usedPercent(drive: Drive): number {
  return drive.total > 0 ? Math.round(((drive.total - drive.free) / drive.total) * 100) : 0
}
</script>

<template>
  <div class="grid grid-cols-[repeat(auto-fill,minmax(14rem,1fr))] gap-3">
    <button
      v-for="drive in drives"
      :key="drive.path"
      type="button"
      class="flex min-w-0 items-start gap-3 rounded-lg border border-stroke bg-card px-4 py-3 text-left transition-colors hover:bg-card-hover disabled:opacity-60"
      :disabled="disabled"
      :title="`Scan ${drive.path}`"
      @click="emit('pick', drive)"
    >
      <component
        :is="drive.removable ? Usb : HardDrive"
        class="mt-0.5 size-6 shrink-0 text-muted"
        :stroke-width="1.5"
      />
      <span class="min-w-0 flex-1">
        <span class="block truncate font-semibold">
          {{ drive.label }} ({{ drive.path.replace(/\\$/, '') }})
        </span>
        <span
          class="mt-2 block h-1.5 overflow-hidden rounded-full bg-subtle-strong"
          role="progressbar"
          :aria-valuenow="usedPercent(drive)"
          aria-valuemin="0"
          aria-valuemax="100"
          :aria-label="`${String(usedPercent(drive))}% used`"
        >
          <span
            class="block h-full rounded-full"
            :class="usedPercent(drive) >= 90 ? 'bg-danger' : 'bg-accent'"
            :style="{ width: `${String(usedPercent(drive))}%` }"
          />
        </span>
        <span class="mt-1 block text-xs text-muted tabular-nums">
          {{ formatBytes(drive.free) }} free of {{ formatBytes(drive.total) }}
        </span>
      </span>
    </button>
  </div>
</template>
