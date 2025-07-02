import { defineConfig } from 'vite'
import inject from "@rollup/plugin-inject";
import vue from '@vitejs/plugin-vue'

// TODO: make sure that is sticky to the client so that multiple people could access the dev server.
let cookie = null;

export default defineConfig({
   transpileDependencies: ["bootstrap-material-design"],
   build: {
      commonjsOptions: { transformMixedEsModules: true },
   },
   plugins: [
      vue(),

      {
         name: "inject-me-build",
         apply: "build",
         transformIndexHtml(_html) {
            return [{
               injectTo: "head",
               tag: "script",
               children: "var me = {{ me }}; var issuers = {{ issuers }};"
            }];
         }
      },
      {
         name: 'configure-server',
         apply: "serve",
         configureServer(server) {
            server.middlewares.use((req, _res, next) => {

               if (req.originalUrl == "/") {
                  // TODO: make sure that is sticky to the client so that multiple people could access the dev server.
                  cookie = req.headers.cookie;
               }

               next();
            })
         },
      },
      {
         name: "inject-me",
         apply: "serve",
         async transformIndexHtml(_html) {
            const meResponse = await fetch("http://127.0.0.1:8000/auth/me", {
               headers: {
                  "Accept": "application/json",
                  "Cookie": cookie,
               }
            })

            let me = null;
            if (meResponse.status == 200) {
               me = await meResponse.json();
            }

            const issuersResponse = await fetch("http://127.0.0.1:8000/auth/issuers", {
               headers: {
                  "Accept": "application/json",
               }
            })

            let issuers = null;
            if (issuersResponse.status == 200) {
               issuers = await issuersResponse.json();
            }

            return [{
               injectTo: "head",
               tag: "script",
               children: `var me = ${JSON.stringify(me)}; var issuers = ${JSON.stringify(issuers)}`
            }];
         }
      },

      inject({
         $: 'jquery',
         jQuery: 'jquery',
         Popper: ['popper.js', 'default']
      })
   ],
   server: {
      port: 9001,
      proxy: {
         '/auth': {
            target: 'http://127.0.0.1:8000'
         },
         '/api': {
            target: 'http://127.0.0.1:8000'
         },
         '/openapi.yaml': {
            target: 'http://127.0.0.1:8000'
         }
      }
   }
})
