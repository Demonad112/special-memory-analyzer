import { createApp } from 'vue'
import App from './App.vue'
import { startShell } from './lib/appInfo'
import { initTheme } from './lib/theme'
import { router } from './router'
import './styles.css'

initTheme()
createApp(App).use(router).mount('#app')
void startShell()
