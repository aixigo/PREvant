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
   <dlg ref="dialog" :title="'Duplicate ' + duplicateFromAppName">
      <template v-slot:body>
         <div class="form-group">
            <input
                  type="name"
                  class="form-control"
                  placeholder="Enter app name"
                  v-model.trim="newAppName"
                  :disabled="!hasWritePermissions"
                  @keyup="keyPressed">
         </div>
         <div v-if="!hasWritePermissions"class="alert alert-warning text-center" role="alert">
            To duplicate an app you need to be logged in.
         </div>
      </template>
      <template v-slot:footer>
         <button
               type="button"
               class="btn btn-outline-primary"
               :disabled="!hasWritePermissions || newAppName.length === 0"
               @click="duplicateApp()">
            Duplicate
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
            newAppName: ''
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
            this.newAppName = '';
            this.$refs.dialog.open();
         },

         keyPressed(e) {
            if (e.keyCode === 13) {
               this.duplicateApp();
            }
         },

         duplicateApp() {
            this.$store.dispatch( 'duplicateApp', {
               appToDuplicate: this.duplicateFromAppName,
               newAppName: this.newAppName
            } );
            this.$refs.dialog.close();
         }
      }
   }
</script>
