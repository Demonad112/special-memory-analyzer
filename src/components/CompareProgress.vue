<script setup lang="ts">
import { CircleCheck, LoaderCircle } from '@lucide/vue'
import type { CompareProgress, PermissionsProgress } from '@/lib/compare'
import { formatBytes, formatCount, plural } from '@/lib/format'

defineProps<{
  progress: CompareProgress | null
  elapsed: string
  /** Set once both folders are read and the permissions pass has started. */
  permissions?: PermissionsProgress | null
}>()
</script>

<template>
  <section
    class="mt-6 rounded-lg border border-stroke bg-card px-5 py-4"
    aria-live="polite"
  >
    <div class="flex items-center justify-between gap-4">
      <p class="flex items-center gap-2 font-semibold">
        <LoaderCircle class="size-4 animate-spin text-accent" />
        {{ permissions ? 'Comparing permissions…' : 'Reading both folders…' }}
      </p>
      <p class="text-xs text-muted tabular-nums">{{ elapsed }} · Esc to cancel</p>
    </div>
    <p
      v-if="permissions"
      class="mt-2 text-xs text-muted tabular-nums"
    >
      {{ formatCount(permissions.done) }} of {{ formatCount(permissions.total) }} items read
    </p>
    <div class="indeterminate mt-3 h-1 overflow-hidden rounded-full bg-subtle-strong" />
    <div class="mt-3 grid grid-cols-2 gap-6">
      <div
        v-for="side in ['left', 'right'] as const"
        :key="side"
        class="min-w-0"
      >
        <p class="flex items-center gap-1.5 text-xs text-muted">
          {{ side === 'left' ? 'Source' : 'Destination' }}
          <span
            v-if="progress?.[side === 'left' ? 'leftDone' : 'rightDone']"
            class="flex items-center gap-1 text-success"
          >
            <CircleCheck class="size-3.5" />
            Done
          </span>
        </p>
        <p class="tabular-nums">
          {{ plural(progress?.[side].files ?? 0, 'file') }} ·
          {{ formatBytes(progress?.[side].bytes ?? 0) }}
        </p>
        <p
          class="truncate text-xs text-faint"
          :title="progress?.[side].current"
        >
          {{ progress?.[side].current || ' ' }}
        </p>
      </div>
    </div>
  </section>
</template>

<style scoped>
/* Fluent indeterminate progress: a short accent bar sliding across. */
.indeterminate {
  position: relative;
}

.indeterminate::after {
  position: absolute;
  inset: 0 auto 0 0;
  width: 30%;
  border-radius: 999px;
  background: var(--app-accent);
  animation: slide 1.4s ease-in-out infinite;
  content: '';
}

@keyframes slide {
  from {
    transform: translateX(-100%);
  }

  to {
    transform: translateX(340%);
  }
}

@media (prefers-reduced-motion: reduce) {
  .indeterminate::after {
    animation-duration: 4s;
  }
}
</style>
