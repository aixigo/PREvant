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
      <a class="navbar-brand" href="#">PREvant</a>
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
                   @keyup="fireSearchEvent">

            <a class="btn btn-outline-success my-2 my-sm-0"
               href="https://github.com/aixigo/PREvant" target="_blank">
               <font-awesome-icon icon="code"/>
               Code
            </a>
            <a class="btn btn-outline-success my-2 my-sm-0" v-if="swaggerUrl !== ''"
               :href="'/swagger-ui/?url=' + swaggerUrl" target="_blank">
               <font-awesome-icon icon="terminal"/>
               API
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

   export default {

      methods: {
         fireSearchEvent() {
            this.$store.commit( 'filterByAppName', this.$refs.searchApps.value );
         }
      }
   }
</script>
