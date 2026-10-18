import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vite'

// https://vite.dev/config/
export default defineConfig({
  base: '/ui/',
  plugins: [vue()],
  build: {
    outDir: '../Ouput/ui',
    emptyOutDir: true,
  },
})
