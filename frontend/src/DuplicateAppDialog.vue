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
   <dlg ref="dialog" :title="'Duplicate ' + duplicateFromAppName" :error-status="errorStatus" :error-status-text="errorStatusText">
      <template slot="body">
         <div class="form-group">
            <input
                  type="name"
                  class="form-control"
                  placeholder="Enter app name"
                  v-model="newAppName"
                  :disabled="noInteraction ? true : false"
                  @keyup="keyPressed">
         </div>
      </template>
      <template slot="footer">
         <button
               type="button"
               class="btn btn-outline-primary"
               @click="duplicateApp()"
               :disabled="noInteraction">
            <font-awesome-icon icon="spinner" spin v-if="noInteraction"/> Duplicate
         </button>
      </template>
   </dlg>
</template>

<script>
   import Dialog from './Dialog.vue';

   export default {
      data() {
         return {
            newAppName: '',
            noInteraction: false,
            errorStatus: null,
            errorStatusText: null,
         };
      },
      components: {
         'dlg': Dialog
      },
      props: {
         duplicateFromAppName: {type: String}
      },
      methods: {
         open() {
            this.$refs.dialog.open();
         },

         keyPressed(e) {
            if (e.keyCode === 13) {
               this.duplicateApp();
            }
         },

         duplicateApp() {
            this.noInteraction = true;

            this.$http.post(`/api/apps/${this.newAppName}?replicateFrom=${this.duplicateFromAppName}`, JSON.stringify([]))
               .then(r => {
                  this.noInteraction = false;
                  this.$refs.dialog.close();

                  // TODO: use a vue.js event to refetch all apps
                  location.reload();
               }, err => {
                  this.noInteraction = false;
                  this.errorStatus = err.status;
                  this.errorStatusText = err.statusText;
               });
         }
      }
   }
</script>