import 'mdn-polyfills/Object.assign';
import 'mdn-polyfills/Object.entries';
import 'mdn-polyfills/String.prototype.endsWith';
import 'isomorphic-fetch';
// Bootstrap material design library
import $ from 'jquery/dist/jquery.js';
import 'popper.js/dist/popper.js';
import 'bootstrap-material-design/dist/css/bootstrap-material-design.css';
import 'bootstrap-material-design/dist/js/bootstrap-material-design.js';

$(document).ready(() => {
    $('body').bootstrapMaterialDesign();
});

import Vue from 'vue';
import VueResource from 'vue-resource';
import VTooltip from 'v-tooltip';

import './scss/theme.scss';
import App from './App.vue';
import Navbar from './Navbar.vue';

import {library} from '@fortawesome/fontawesome-svg-core';
import {faCode, faServer, faSpinner, faTerminal} from '@fortawesome/free-solid-svg-icons';
import {FontAwesomeIcon} from '@fortawesome/vue-fontawesome';
import store from './store';

library.add(faCode);
library.add(faServer);
library.add(faSpinner);
library.add(faTerminal);

Vue.use(VueResource);
Vue.use(VTooltip);
Vue.component('font-awesome-icon', FontAwesomeIcon);

store.dispatch('fetchData');

new Vue({
    el: '#app',
    store,
    render: h => h(App)
});

new Vue({
    el: '#nav',
    store,
    render: h => h(Navbar)
});


