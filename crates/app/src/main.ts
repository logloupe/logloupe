import { createApp } from 'vue'
import { createRouter, createMemoryHistory } from 'vue-router'
import ui from '@nuxt/ui/vue-plugin'
import { addCollection } from '@iconify/vue'
import lucide from '@iconify-json/lucide/icons.json'
import App from './App.vue'
import './assets/main.css'
import 'uplot/dist/uPlot.min.css'

// Icons are bundled with the app. Without this they would be fetched from a
// public icon API at run time, and the app must make no such requests.
addCollection(lucide)

// Nuxt UI's links expect a router. The app itself switches views in state.
const router = createRouter({
  history: createMemoryHistory(),
  routes: [{ path: '/', component: { render: () => null } }],
})

createApp(App).use(router).use(ui).mount('#app')
