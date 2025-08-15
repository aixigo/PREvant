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
   <transition name="modal">
      <div class="ra-modal-mask">
         <div class="ra-modal-wrapper">
            <div class="ra-modal-container">

               <div class="ra-modal-header">
                  <h1>API Documentation<span v-if="showAdditionalHeadlineInformation"> – {{ $route.params.title }}</span></h1>

                  <span @click="close()">
                     <font-awesome-icon icon="window-close" class="ra-modal-close-button" aria-label="Close"/>
                  </span>
               </div>

               <div class="ra-modal-body">
                  <div :id="`swagger-ui-${this.$.uid}`"></div>
               </div>
            </div>
         </div>
      </div>
   </transition>
</template>

<style type="text/css">
   @import "swagger-ui/dist/swagger-ui.css";

   /* Fixes swagger ui response column width */
   .ra-modal-container .col.response-col_status {
       width: 10% !important;
   }
   .ra-modal-container .col_header.parameters-col_name {
       width: 10% !important;
   }
   .ra-modal-container .col.parameters-col_name {
       width: 10% !important;
   }

   .ra-modal-close-button {
       position: absolute;
       right: 20px;
       top: 20px;
   }

   .ra-modal-mask {
      position: fixed;
      z-index: 9998;
      top: 0;
      left: 0;
      width: 100%;
      height: 100%;
      background-color: rgba(0, 0, 0, .5);
      display: table;
      transition: opacity .3s ease;
   }

   .ra-modal-wrapper {
      display: table-cell;
      vertical-align: middle;
   }

   .ra-modal-container {
      position: fixed;
      top: 50px;
      left: 50px;
      bottom: 50px;
      right: 50px;
      padding: 20px 30px;
      background-color: #fff;
      border-radius: 2px;
      box-shadow: 0 2px 8px rgba(0, 0, 0, .33);
      transition: all .3s ease;
      font-family: Helvetica, Arial, sans-serif;

      overflow-y: scroll;
   }

   .ra-modal-header h3 {
      margin-top: 0;
      color: #42b983;
   }

   .ra-modal-body {
      margin: 20px 0;
   }

   .ra-modal-enter .ra-modal-container,
   .ra-modal-leave-active .ra-modal-container {
      -webkit-transform: scale(0.1);
      transform: scale(0.1);
   }
</style>

<script>
   import SwaggerUI from 'swagger-ui';

   export default {
      data() {
         return {};
      },
      computed: {
         showAdditionalHeadlineInformation() {
            return this.$route.params.title != null;
         }
      },
      mounted() {
         window.addEventListener("keydown", this.handleKeydown);

         SwaggerUI( {
            url: this.$route.params.url,
            dom_id: `#swagger-ui-${this.$.uid}`
         } );
      },
      beforeUnmount() {
         window.removeEventListener("keydown", this.handleKeydown);
      },
      methods: {
         close() {
           this.$router.push(this.$router.options.history.state.back ?? "/");
         },
         handleKeydown(event) {
            if (event.key === "Escape") {
               this.close();
            }
         }
      }
   }
</script>

