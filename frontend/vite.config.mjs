import { defineConfig } from 'vite'
import inject from "@rollup/plugin-inject";
import vue from '@vitejs/plugin-vue'
import vueDevTools from 'vite-plugin-vue-devtools'
import path from 'path';
import fs from 'fs';

// TODO: make sure that is sticky to the client so that multiple people could access the dev server.
let cookie = null;

export default defineConfig({
   transpileDependencies: ["bootstrap-material-design"],
   build: {
      commonjsOptions: { transformMixedEsModules: true },
   },
   plugins: [
      vue(),
      vueDevTools(),

      {
         name: "inject-template-vars",
         apply: "build",
         transformIndexHtml(_html) {
            return [
               {
                  injectTo: "head",
                  tag: "script",
                  children: `
                     var me = {{ me }};
                     var issuers = {{ issuers }};
                     var config = {{ config }};
                  `
               },
               {
                  injectTo: "head",
                  tag: "title",
                  children: "{{ title }}"
               }
            ];
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
         name: "inject-runtime-context",
         apply: "serve",
         async transformIndexHtml(_html) {
            const me= await fetch("http://127.0.0.1:8000/auth/me", {
                  headers: {
                     "Accept": "application/json",
                     "Cookie": cookie,
                  }
               })
               .then(res => {
                  if (res.status == 200) {
                     return res.json();
                  }
                  return null;
               })
               .catch(() => null);

            const issuers = await fetch("http://127.0.0.1:8000/auth/issuers", {
                  headers: {
                     "Accept": "application/json",
                  }
               })
               .then(res => {
                  if (res.status == 200) {
                     return res.json();
                  }
                  return null;
               })
               .catch(() => null);

            return [
               {
                  injectTo: "head",
                  tag: "script",
                  children: `
                     var me = ${JSON.stringify(me)};
                     var issuers = ${JSON.stringify(issuers)};
                     var config = { defaultAppName: 'master' };
                  `
               },
               {
                  injectTo: "head",
                  tag: "title",
                  children: "PREvant (dev)"
               }
            ];
         }
      },

      /**
       * This plugin serves fixture files (e.g., AsyncAPI YAMLs) during development only.
       * It maps requests to /fixtures/... to local files under tests/fixtures.
       * The files are not included in the production build and are only used for
       * development/testing purposes.
       */
      {
         name: 'serve-fixtures-dev-only',
         apply: "serve",
         configureServer(server) {
            server.middlewares.use((req, res, next) => {
               if (req.url?.startsWith('/fixtures/')) {
                  const filePath = path.join(__dirname, 'tests/fixtures', req.url.replace('/fixtures/', ''));
                  if (fs.existsSync(filePath)) {
                     res.setHeader('Content-Type', 'application/octet-stream');
                     fs.createReadStream(filePath).pipe(res);
                     return;
                  }
                  res.statusCode = 404;
                  res.end('Not Found');
                  return;
               }
               next();
            });
         },
      },

      inject({
         $: 'jquery',
         jQuery: 'jquery',
         Popper: ['popper.js', 'default'],

         // Exclude CSS files, especially those imported by Swagger UI (swagger-ui.css),
         // because rollup-plugin-inject tries to parse all files as JavaScript.
         // Parsing CSS as JS causes warnings like:
         //   "rollup-plugin-inject: failed to parse ...swagger-ui.css?... Consider restricting the plugin"
         exclude: ['**/*.css']
      })
   ],
   server: {
      port: 9001,
      proxy: {
         '/auth': {
            target: `http://127.0.0.1:${process.env.API_PORT ?? 8000}`
         },
         '/api': {
            target: `http://127.0.0.1:${process.env.API_PORT ?? 8000}`
         },
         '/openapi.yaml': {
            target: `http://127.0.0.1:${process.env.API_PORT ?? 8000}`
         }
      }
   }
})
