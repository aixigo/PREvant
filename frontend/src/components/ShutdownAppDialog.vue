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
   <dlg ref="dialog" :title="'Shutdown ' + appName" @opened="focusInput">
      <template v-slot:body>
         <p>Do you really want to shutdown <b>{{ appName }}</b>? Confirm by typing in the app:</p>

         <div class="form-group">
            <input
                  ref="confirmedAppNameInput"
                  type="name"
                  class="form-control"
                  placeholder="Enter app name"
                  v-model="confirmedAppName"
                  :disabled="!hasWritePermissions"
                  @keyup="keyPressed">
         </div>

         <div v-if="!hasWritePermissions" class="alert alert-warning text-center" role="alert">
            You need to be logged in to shutdown apps.
         </div>
      </template>
      <template v-slot:footer>
         <button
               type="button"
               class="btn btn-outline-danger"
               @click="deleteApp()"
               :disabled="!hasWritePermissions || confirmedAppName !== appName">
            Confirm
         </button>
      </template>
   </dlg>
</template>

<script>
   import { useAuth } from '../composables/useAuth';
   import Dialog from './Dialog.vue';

   export default {
      setup() {
         const { hasWritePermissions } = useAuth();

         return {
            hasWritePermissions
         };
      },
      data() {
         return {
            confirmedAppName: ''
         }
      },
      components: {
         'dlg': Dialog
      },
      props: {
         appName: {type: String}
      },
      methods: {
         open() {
            this.confirmedAppName = '';
            this.$refs.dialog.open();
         },
         focusInput() {
            const input = this.$refs.confirmedAppNameInput;
            if (input && !input.disabled) {
               input.focus();
            }
         },
         keyPressed(e) {
            if (e.keyCode === 13) {
               this.deleteApp();
            }
         },
         deleteApp() {
            if (this.confirmedAppName !== this.appName) {
               return;
            }

            this.$store.dispatch( 'deleteApp', { appName: this.confirmedAppName } );
            this.$refs.dialog.close();
         }
      }
   }
</script>
