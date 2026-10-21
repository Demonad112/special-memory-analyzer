<script setup lang="ts">
import { computed } from 'vue'
import { idleWarningMs, stayOpen } from '@/lib/idleTimeout'

const seconds = computed(() => Math.max(0, Math.ceil((idleWarningMs.value ?? 0) / 1000)))
</script>

<template>
  <div
    v-if="idleWarningMs !== null"
    class="fixed inset-0 z-50 flex items-center justify-center bg-fg/20"
  >
    <div
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="idle-warning-title"
      aria-describedby="idle-warning-body"
      class="w-[min(28rem,92vw)] rounded-lg border border-stroke-strong bg-app p-5 text-fg shadow-xl"
    >
      <h2
        id="idle-warning-title"
        class="font-semibold"
      >
        Closing for inactivity
      </h2>
      <p
        id="idle-warning-body"
        class="mt-2 text-[13px] text-muted"
      >
        Deep Server Analyzer closes in {{ seconds }} s because nothing has happened for a while.
        Nothing is closed while a job is running.
      </p>
      <div class="mt-5 flex justify-end">
        <button
          class="h-8 rounded-md bg-accent px-4 font-semibold text-on-accent hover:bg-accent-hover"
          autofocus
          @click="stayOpen"
        >
          Stay open
        </button>
      </div>
    </div>
  </div>
</template>
