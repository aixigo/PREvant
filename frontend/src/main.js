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
// Bootstrap material design library
import $ from 'jquery';
import 'popper.js';
import 'bootstrap-material-design/dist/css/bootstrap-material-design.css';
import 'bootstrap-material-design/dist/js/bootstrap-material-design.js';

$(document).ready(() => {
     $('body').bootstrapMaterialDesign();
});

import { createApp, } from 'vue';
import { createRouter, createWebHashHistory } from 'vue-router';

import './scss/theme.scss';
import Main from './Main.vue';
import Apps from './views/Apps.vue';
import AsyncApiUI from './views/AsyncApiUI.vue';
import OpenApiUI from './views/OpenApiUI.vue';
import LogsDialog from './views/LogsDialog.vue';

import {library} from '@fortawesome/fontawesome-svg-core';
import {faClipboard, faCode, faCopy, faServer, faSpinner, faTerminal, faTrash, faWindowClose, faDownload} from '@fortawesome/free-solid-svg-icons';
import {FontAwesomeIcon} from '@fortawesome/vue-fontawesome';
import { createStore } from './store';

library.add(faClipboard);
library.add(faCode);
library.add(faCopy);
library.add(faDownload);
library.add(faServer);
library.add(faSpinner);
library.add(faTerminal);
library.add(faTrash);
library.add(faWindowClose);

export const router = createRouter({
   history: createWebHashHistory(),
   // It is currently not possible to use lazy loading for routes because of bootstrap v4 and jquery
   routes: [
      { path: '/', component: Apps, query: { appNameFilter: { type: String } } },
      { path: '/open-api-ui/:url', name: 'open-api-ui', component: OpenApiUI },
      { path: '/async-api-ui/:url', name: 'async-api-ui', component: AsyncApiUI },
      { path: '/logs/:app/:service', name: 'logs', component: LogsDialog }
   ]
});

// Please note, that me and issuers are injected by the dev server or by the PREvant backend.
const store = createStore(router, me, issuers);
store.dispatch('fetchData');

createApp(Main)
   .component('font-awesome-icon', FontAwesomeIcon)
   .use(store)
   .use(router)
   .mount('#main')

