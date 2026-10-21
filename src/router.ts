import { FileText, FolderSync, HardDrive, House, Settings } from '@lucide/vue'
import type { Component } from 'vue'
import { createRouter, createWebHistory } from 'vue-router'
import CleanupPage from './pages/CleanupPage.vue'
import ComparePage from './pages/ComparePage.vue'
import HomePage from './pages/HomePage.vue'
import ReportsPage from './pages/ReportsPage.vue'
import RecordPage from './pages/RecordPage.vue'
import RecordReportPage from './pages/RecordReportPage.vue'
import SettingsPage from './pages/SettingsPage.vue'

export interface Section {
  path: string
  title: string
  summary: string
  icon: Component
}

/** The app's sections, in nav order. Settings sits at the bottom of the rail. */
export const sections: Section[] = [
  { path: '/', title: 'Home', summary: 'Start here.', icon: House },
  {
    path: '/compare',
    title: 'Compare & Record',
    summary:
      'Compare two folders by size, then record a copy between them and see every file that did not make it.',
    icon: FolderSync,
  },
  {
    path: '/cleanup',
    title: 'Disk Cleanup',
    summary: 'See what fills a drive and free space safely.',
    icon: HardDrive,
  },
  {
    path: '/reports',
    title: 'Reports',
    summary: 'Every compare, copy and cleanup report, ready to export for the client.',
    icon: FileText,
  },
]

export const settingsSection: Section = {
  path: '/settings',
  title: 'Settings',
  summary: 'Appearance and defaults.',
  icon: Settings,
}

export const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', component: HomePage },
    { path: '/compare', component: ComparePage },
    { path: '/compare/record', component: RecordPage },
    { path: '/compare/record/:id', component: RecordReportPage, props: true },
    { path: '/cleanup', component: CleanupPage },
    { path: '/reports', component: ReportsPage },
    { path: '/settings', component: SettingsPage },
    { path: '/:pathMatch(.*)*', redirect: '/' },
  ],
})
