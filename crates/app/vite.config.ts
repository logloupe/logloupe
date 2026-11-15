import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import ui from '@nuxt/ui/vite'

// Tauri expects a fixed port and does not need the screen cleared.
export default defineConfig({
  plugins: [
    vue(),
    ui({
      ui: { colors: { primary: 'teal', neutral: 'slate' } },
    }),
  ],
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ['**/src-tauri/**'] } },
  build: { target: 'es2022', chunkSizeWarningLimit: 1500 },
})
