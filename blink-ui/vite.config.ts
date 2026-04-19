import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import path from 'path'

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  server: {
    port: 5173,
    proxy: {
      '/api': {
        target: 'http://localhost:3030',
        changeOrigin: true,
      },
      '/ws': {
        target: 'ws://localhost:3030',
        ws: true,
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: '../blink-engine/static/ui',
    emptyOutDir: true,
    rollupOptions: {
      output: {
        manualChunks: {
          icons: ['lucide-react'],
          charts: ['recharts'],
          motion: ['motion'],
          radix: [
            '@radix-ui/react-dialog',
            '@radix-ui/react-dropdown-menu',
            '@radix-ui/react-scroll-area',
            '@radix-ui/react-separator',
            '@radix-ui/react-slot',
            '@radix-ui/react-tabs',
            '@radix-ui/react-tooltip',
          ],
        },
      },
    },
  },
})
