import { defineConfig } from 'vite'
import inject from "@rollup/plugin-inject";
import vue from '@vitejs/plugin-vue'

export default defineConfig({
   transpileDependencies: ["bootstrap-material-design"],
   build: {
      commonjsOptions: { transformMixedEsModules: true },
   },
   plugins: [
      vue(),

      inject({
         $: 'jquery',
         jQuery: 'jquery',
         Popper: ['popper.js', 'default']
      })
   ],
   server: {
      port: 9001,
      proxy: {
         '/api': {
            target: 'http://127.0.0.1:8000'
         },
         '/openapi.yaml': {
            target: 'http://127.0.0.1:8000'
         }
      }
   }
})
