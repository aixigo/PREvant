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
   <MDBNavbar expand="lg" light bg="light" position="top" container>
      <MDBNavbarBrand href="#">
         <img src="/logo.svg" alt="PREvant Logo" height="30" loading="lazy" />
      </MDBNavbarBrand>

      <MDBNavbarToggler target="#navbarSupportedContent" @click="collapse = !collapse" />

      <MDBCollapse v-model="collapse" id="navbarSupportedContent">
         <MDBNavbarNav class="mb-2 mb-lg-0 align-items-center" right>
            <form class="ra-navbar-search d-flex align-items-center mb-0">
               <MDBInput
                  type="search"
                  wrapperClass="me-2 mb-0 w-auto ra-navbar-search-input"
                  placeholder="Search Apps"
                  aria-label="Search"
                  :model-value="appNameFilter"
                  @update:model-value="fireSearchEvent">
                  <MDBIcon icon="magnifying-glass" class="trailing text-muted" />
               </MDBInput>
            </form>

            <MDBBtn outline="success" tag="a" href="https://github.com/aixigo/PREvant" target="_blank"
               class="me-2 mb-0 border-0 d-flex align-items-center">
               <MDBIcon icon="code" class="me-1" />
               Code
            </MDBBtn>

            <MDBBtn outline="success" tag="router-link"
               :to="{ name: 'open-api-ui', params: { url: '/openapi.yaml' } }"
               class="me-2 mb-0 border-0 d-flex align-items-center">
               <MDBIcon icon="terminal" class="me-1" />
               API
            </MDBBtn>

            <span v-if="me" class="ms-2">
               {{ name }}
            </span>

            <MDBBtn v-else-if="issuers != null" v-for="issuer in issuers" :key="issuer.issuer" outline="success"
               tag="a" class="ms-2" :href="issuer.loginUrl">
               Login with {{ issuer.issuer }}
            </MDBBtn>
         </MDBNavbarNav>
      </MDBCollapse>
   </MDBNavbar>
</template>

<script>
   import { ref } from 'vue';
   import {
      MDBNavbar,
      MDBNavbarBrand,
      MDBNavbarToggler,
      MDBNavbarNav,
      MDBCollapse,
      MDBBtn,
      MDBInput,
      MDBIcon
   } from "mdb-vue-ui-kit";
   import { mapGetters } from 'vuex';

   export default {
      components: {
         MDBNavbar,
         MDBNavbarBrand,
         MDBNavbarToggler,
         MDBNavbarNav,
         MDBCollapse,
         MDBBtn,
         MDBInput,
         MDBIcon,
      },
      data() {
         return {};
      },
      setup() {
         const collapse = ref(false);

         return {
            collapse
         }
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
         fireSearchEvent(newAppNameFilter) {
            this.$store.commit( 'filterByAppName', newAppNameFilter);
         }
      }
   }
</script>

<style scoped>
.ra-navbar-search :deep(.form-outline) {
   margin-bottom: 0;
}
</style>
