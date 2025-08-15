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

<template>
   <nav id="nav" class="navbar navbar-expand-lg navbar-light bg-light">
      <a class="navbar-brand" href="#">
         <svg xmlns="http://www.w3.org/2000/svg" width="100" height="29.253" viewBox="0 0 26.458 7.74">
            <path class="cls-34" fill="#333" d="M0 0h26.458v7.74H0z"/>
            <path d="M3.443 4.52v1.858h-.62v-4.75h1.518a1.38 1.38 0 011.046.397 1.467 1.467 0 01.378 1.059 1.474 1.474 0 01-.36 1.052 1.393 1.393 0 01-1.021.384zm0-.514h.922a.793.793 0 00.62-.235 1.003 1.003 0 00.21-.687 1.022 1.022 0 00-.217-.682.75.75 0 00-.619-.26h-.916zM8.006 4.458h-.848v1.92h-.62v-4.75H7.87a1.443 1.443 0 011.065.366 1.437 1.437 0 01.36 1.065 1.474 1.474 0 01-.193.768 1.238 1.238 0 01-.557.495l.929 2.019v.037h-.65zm-.848-.514h.724a.774.774 0 00.62-.241.892.892 0 00.229-.62c0-.619-.279-.916-.836-.916h-.737zM12.415 4.198h-1.69V5.87h1.969v.508H10.13v-4.75h2.526v.514h-1.931v1.53h1.69zM14.297 5.505l.669-2.657h.588l-1.046 3.53h-.434l-1.065-3.53h.589zM17.932 6.378a1.53 1.53 0 01-.074-.384.91.91 0 01-.787.452.941.941 0 01-1.021-1.01 1.077 1.077 0 01.365-.854 1.505 1.505 0 011.01-.322h.42v-.378a.49.49 0 00-.57-.582.57.57 0 00-.42.155.508.508 0 00-.168.39h-.575a.885.885 0 01.16-.514 1.084 1.084 0 01.428-.39 1.325 1.325 0 01.619-.136 1.164 1.164 0 01.83.266 1.071 1.071 0 01.29.786v1.765a2.037 2.037 0 00.112.706v.05zm-.774-.458a.762.762 0 00.409-.118.694.694 0 00.278-.291v-.842h-.322a1.09 1.09 0 00-.656.18.62.62 0 00-.235.5.62.62 0 00.117.434.514.514 0 00.409.136zM19.87 2.848v.384a1.046 1.046 0 01.867-.446q.935 0 .947 1.239v2.347h-.576v-2.31a.941.941 0 00-.136-.62.508.508 0 00-.42-.179.563.563 0 00-.385.149.947.947 0 00-.278.371v2.564h-.582v-3.53zM23.325 1.994v.854h.551v.47h-.532v2.193a.62.62 0 00.068.316.266.266 0 00.235.105.78.78 0 00.235-.037v.483a1.276 1.276 0 01-.402.068.62.62 0 01-.526-.248 1.152 1.152 0 01-.18-.687V3.319h-.539v-.47h.54v-.855z" class="cls-14" fill="#fff"/>
         </svg>
      </a>
      <button class="navbar-toggler" type="button" data-toggle="collapse" data-target="#navbarSupportedContent"
              aria-controls="navbarSupportedContent" aria-expanded="false" aria-label="Toggle navigation">
         <span class="navbar-toggler-icon"></span>
      </button>

      <div class="collapse navbar-collapse" id="navbarSupportedContent">
         <ul class="navbar-nav mr-auto">
         </ul>
         <form class="form-inline my-2 my-lg-0">

            <input class="form-control mr-sm-2" type="search" placeholder="Search Apps" aria-label="Search"
                   ref="searchApps"
                   :value="appNameFilter"
                   @input="fireSearchEvent">

            <a class="btn btn-outline-success my-2 my-sm-0"
               href="https://github.com/aixigo/PREvant" target="_blank">
               <font-awesome-icon icon="code"/>
               Code
            </a>
            <router-link class="btn btn-outline-success my-2 my-sm-0" :to="{ name: 'open-api-ui', params: { url: '/openapi.yaml' }, meta: { title: 'PREvant' }}">
               <font-awesome-icon icon="terminal"/>
               API
            </router-link>

            <a href="#" class="my-2 my-sm-0" v-if="me">
               {{ name }}
            </a>
            <a class="btn btn-outline-success my-2 my-sm-0"
               v-else-if="issuers != null" v-for="issuer in issuers" :href="issuer.loginUrl">
               Login with {{ issuer.issuer }}
            </a>
         </form>
      </div>
   </nav>

</template>

<style>
   nav > div > form.form-inline > a {
      margin-left: 10px;
   }
</style>

<script>
   import { mapGetters } from 'vuex';
   import OpenApiUI from './OpenApiUI.vue';

   export default {
      data() {
         return {};
      },
      components: {
         'open-api-ui': OpenApiUI
      },
      computed: {
         ...mapGetters( [ 'appNameFilter', 'me', 'issuers' ] ),

         name() {
            if (me.name != null) {
               return me.name;
            }
            return me.sub;
         }
      },
      methods: {
         fireSearchEvent(e) {
            this.$store.commit( 'filterByAppName', e.target.value);
         }
      }
   }
</script>
