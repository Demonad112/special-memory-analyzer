<script setup lang="ts">
import { ArrowRight, ChevronRight, History, ListChecks } from '@lucide/vue'
import { computed, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import DriveTiles from '@/components/DriveTiles.vue'
import PageHeader from '@/components/PageHeader.vue'
import { appInfo } from '@/lib/appInfo'
import { drives, refreshDrives, type Drive } from '@/lib/cleanup'
import { loadRecent, savePaths, type ComparePaths } from '@/lib/compare'
import { sections } from '@/router'

const router = useRouter()
const recent = loadRecent()

/** Opens Compare with this pair filled in and starts it. */
function compareAgain(pair: ComparePaths): void {
  savePaths(pair)
  void router.push({ path: '/compare', query: { run: '1' } })
}

/** A drive tile opens Disk Cleanup and scans that drive. */
function scanDrive(drive: Drive): void {
  void router.push({ path: '/cleanup', query: { scan: drive.path } })
}
onMounted(refreshDrives)

const tools = sections.slice(1)
const summary = computed(() =>
  appInfo.value.machine
    ? `Working on ${appInfo.value.machine}. Pick a tool to begin.`
    : 'Pick a tool to begin.',
)
</script>

<template>
  <div class="mx-auto max-w-5xl px-10 py-8">
    <PageHeader
      title="Welcome to Deep Server Analyzer"
      :summary="summary"
    />
    <div class="grid grid-cols-3 gap-4">
      <RouterLink
        v-for="tool in tools"
        :key="tool.path"
        :to="tool.path"
        class="group flex flex-col rounded-lg border border-stroke bg-card p-5 shadow-xs transition-colors hover:bg-card-hover"
      >
        <span
          class="grid size-11 place-items-center rounded-lg bg-accent/12 text-accent"
          aria-hidden="true"
        >
          <component
            :is="tool.icon"
            class="size-5"
            :stroke-width="1.75"
          />
        </span>
        <span class="mt-4 flex items-center gap-1 font-semibold">
          {{ tool.title }}
          <ChevronRight
            class="size-4 text-faint transition-transform group-hover:translate-x-0.5"
          />
        </span>
        <span class="mt-1 text-[13px] leading-5 text-muted">{{ tool.summary }}</span>
      </RouterLink>
    </div>
    <RouterLink
      to="/compare/readiness"
      class="group mt-4 flex items-center gap-4 rounded-lg border border-stroke bg-card px-5 py-3 shadow-xs transition-colors hover:bg-card-hover"
    >
      <span
        class="grid size-9 shrink-0 place-items-center rounded-lg bg-accent/12 text-accent"
        aria-hidden="true"
      >
        <ListChecks
          class="size-4.5"
          :stroke-width="1.75"
        />
      </span>
      <span class="min-w-0 flex-1">
        <span class="flex items-center gap-1 font-semibold">
          Migration readiness
          <ChevronRight
            class="size-4 text-faint transition-transform group-hover:translate-x-0.5"
          />
        </span>
        <span class="text-[13px] text-muted"
          >Before a move: find names the target cannot hold, paths that will be too long and files
          in use.</span
        >
      </span>
    </RouterLink>

    <section
      v-if="drives.length > 0"
      class="mt-8"
    >
      <h2 class="mb-2 text-[13px] font-semibold text-muted">This PC</h2>
      <DriveTiles
        :drives="drives"
        @pick="scanDrive"
      />
    </section>

    <section
      v-if="recent.length > 0"
      class="mt-8"
    >
      <h2 class="mb-2 flex items-center gap-1.5 text-[13px] font-semibold text-muted">
        <History class="size-4" />
        Recent compares
      </h2>
      <ul class="overflow-hidden rounded-lg border border-stroke bg-card">
        <li
          v-for="pair in recent"
          :key="`${pair.left}|${pair.right}`"
          class="border-b border-stroke last:border-b-0"
        >
          <button
            type="button"
            class="group flex w-full items-center gap-3 px-4 py-2.5 text-left text-[13px] hover:bg-card-hover"
            :title="`Compare ${pair.left} with ${pair.right} again`"
            @click="compareAgain(pair)"
          >
            <span class="min-w-0 flex-1 truncate">{{ pair.left }}</span>
            <ArrowRight class="size-3.5 shrink-0 text-faint" />
            <span class="min-w-0 flex-1 truncate">{{ pair.right }}</span>
            <span class="shrink-0 text-xs text-accent opacity-0 group-hover:opacity-100">
              Compare again
            </span>
          </button>
        </li>
      </ul>
    </section>
  </div>
</template>
