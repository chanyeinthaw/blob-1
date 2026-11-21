import { defineConfig } from 'vite'

import { foldkit } from '@foldkit/vite-plugin'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [tailwindcss(), foldkit()],
  server: {
    proxy: {
      '/api': 'http://127.0.0.1:7878',
      '/ws': { target: 'ws://127.0.0.1:7878', ws: true },
    },
  },
  optimizeDeps: {
    entries: ['src/entry.ts'],
  },
})
