<script setup lang="ts">
import { Archive, Building2, Monitor, Moon, Palette, Sun, Timer } from '@lucide/vue'
import PageHeader from '@/components/PageHeader.vue'
import { appInfo } from '@/lib/appInfo'
import { idleChoices, idleMinutes, setIdleMinutes } from '@/lib/idleTimeout'
import { brand } from '@/lib/job'
import { retentionChoices, retentionDays } from '@/lib/reports'
import { setThemeChoice, themeChoice, type ThemeChoice } from '@/lib/theme'

function idleLabel(minutes: number): string {
  if (minutes === 0) return 'Off'
  if (minutes === 60) return '1 hour'
  if (minutes === 120) return '2 hours'
  return `${minutes} minutes${minutes === 30 ? ' (default)' : ''}`
}

const themes: { value: ThemeChoice; label: string; icon: typeof Sun }[] = [
  { value: 'light', label: 'Light', icon: Sun },
  { value: 'dark', label: 'Dark', icon: Moon },
  { value: 'system', label: 'Use Windows setting', icon: Monitor },
]
</script>

<template>
  <div class="mx-auto max-w-3xl px-10 py-8">
    <PageHeader title="Settings" />
    <section class="flex items-center gap-4 rounded-lg border border-stroke bg-card px-5 py-4">
      <Palette
        class="size-5 shrink-0"
        :stroke-width="1.5"
      />
      <div class="flex-1">
        <p>App theme</p>
        <p class="text-xs text-muted">Choose how Deep Server Analyzer looks.</p>
      </div>
      <div
        class="flex rounded-md border border-stroke bg-subtle p-0.5"
        role="radiogroup"
        aria-label="App theme"
      >
        <button
          v-for="theme in themes"
          :key="theme.value"
          role="radio"
          :aria-checked="themeChoice === theme.value"
          class="flex h-8 items-center gap-2 rounded px-3 text-[13px]"
          :class="
            themeChoice === theme.value
              ? 'bg-accent text-on-accent'
              : 'text-fg hover:bg-subtle-strong'
          "
          @click="setThemeChoice(theme.value)"
        >
          <component
            :is="theme.icon"
            class="size-3.5"
          />
          {{ theme.label }}
        </button>
      </div>
    </section>
    <label class="mt-3 flex items-center gap-4 rounded-lg border border-stroke bg-card px-5 py-4">
      <Building2
        class="size-5 shrink-0"
        :stroke-width="1.5"
      />
      <span class="flex-1">
        <span class="block">Company name on reports</span>
        <span class="block text-xs text-muted">
          Shown at the top of every exported report. Empty: "Deep Server Analyzer".
        </span>
      </span>
      <input
        v-model="brand"
        type="text"
        spellcheck="false"
        autocomplete="organization"
        placeholder="Your company"
        class="h-8 w-60 rounded-md border border-stroke border-b-stroke-strong bg-card px-2.5 text-[13px] outline-none placeholder:text-faint focus:border-b-2 focus:border-b-accent"
      />
    </label>
    <label class="mt-3 flex items-center gap-4 rounded-lg border border-stroke bg-card px-5 py-4">
      <Archive
        class="size-5 shrink-0"
        :stroke-width="1.5"
      />
      <span class="flex-1">
        <span class="block">Keep saved reports</span>
        <span class="block text-xs text-muted">
          Older Compare, Disk usage and Cleanup reports are deleted when you open Reports. Record
          runs are client proof and are never deleted automatically.
        </span>
      </span>
      <select
        v-model.number="retentionDays"
        class="h-8 w-40 rounded-md border border-stroke border-b-stroke-strong bg-card px-2 text-[13px] outline-none focus:border-b-2 focus:border-b-accent"
      >
        <option
          v-for="choice in retentionChoices"
          :key="choice.days"
          :value="choice.days"
        >
          {{ choice.label }}
        </option>
      </select>
    </label>
    <label class="mt-3 flex items-center gap-4 rounded-lg border border-stroke bg-card px-5 py-4">
      <Timer
        class="size-5 shrink-0"
        :stroke-width="1.5"
      />
      <span class="flex-1">
        <span class="block">Close after inactivity</span>
        <span class="block text-xs text-muted">
          Closes the app after no mouse, keyboard or touch for this long. A warning appears one
          minute before. Never closes while a job is running.
        </span>
      </span>
      <select
        :value="idleMinutes"
        class="h-8 w-40 rounded-md border border-stroke border-b-stroke-strong bg-card px-2 text-[13px] outline-none focus:border-b-2 focus:border-b-accent"
        @change="setIdleMinutes(Number(($event.target as HTMLSelectElement).value))"
      >
        <option
          v-for="minutes in idleChoices"
          :key="minutes"
          :value="minutes"
        >
          {{ idleLabel(minutes) }}
        </option>
      </select>
    </label>
    <p class="mt-6 text-xs text-faint">Deep Server Analyzer {{ appInfo.version }}</p>
  </div>
</template>
