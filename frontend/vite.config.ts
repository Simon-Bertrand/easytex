import { defineConfig } from 'vite'
import solid from 'vite-plugin-solid'

export default defineConfig({
  plugins: [solid()],
  server: {
    port: 3000,
    proxy: {
      '/api': 'http://localhost:8081',
      '/events': 'http://localhost:8081',
      '/pdf': 'http://localhost:8081',
    }
  }
})
