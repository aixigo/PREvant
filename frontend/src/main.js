/*-
 * ========================LICENSE_START=================================
 * PREvant Frontend
 * %%
 * Copyright (C) 2018 - 2019 aixigo AG
 * %%
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
 * THE SOFTWARE.
 * =========================LICENSE_END==================================
 */
import { createApp, } from 'vue';
import { createRouter, createWebHashHistory } from 'vue-router';
import '@fontsource/roboto/latin-300.css';
import '@fontsource/roboto/latin-400.css';
import '@fontsource/roboto/latin-500.css';
import '@fontsource/roboto/latin-700.css';
import '@fortawesome/fontawesome-free/css/fontawesome.min.css';
import '@fortawesome/fontawesome-free/css/solid.min.css';
import 'mdb-vue-ui-kit/css/mdb.min.css';
import './scss/theme.scss';
import Main from './Main.vue';
import { createStore } from './store';
export const router = createRouter({
   history: createWebHashHistory(),
   routes: [
      { path: '/:heading?', component: () => import('./views/Apps.vue'), query: { appNameFilter: { type: String } } },
      { path: '/open-api-ui/:url', name: 'open-api-ui', component: () => import('./views/OpenApiUI.vue') },
      { path: '/async-api-ui/:url', name: 'async-api-ui', component: () => import('./views/AsyncApiUI.vue') },
      { path: '/logs/:app/:service', name: 'logs', component: () => import('./views/LogsDialog.vue') }
   ]
});

// Please note, that me and issuers are injected by the dev server or by the PREvant backend.
const store = createStore(router, me, issuers);
store.dispatch('fetchData');

createApp(Main)
   .use(store)
   .use(router)
   .mount('#main')
