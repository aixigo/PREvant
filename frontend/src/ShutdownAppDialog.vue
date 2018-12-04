/*-
 * ========================LICENSE_START=================================
 * PREvant Frontend
 * %%
 * Copyright (C) 2018 aixigo AG
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
   <div ref="dialog" class="modal fade" tabindex="-1" role="dialog">
      <div class="modal-dialog modal-dialog-centered" role="document">
         <div class="modal-content">
            <div class="modal-header">
               <h5 class="modal-title">Shutdown {{ appName }}</h5>
               <button type="button" class="close" data-dismiss="modal" aria-label="Close">
                  <span aria-hidden="true">&times;</span>
               </button>
            </div>
            <div class="modal-body">
               <p>Do you really want to shutdown <b>{{ appName }}</b>? Confirm by typing in the app:</p>

               <div class="form-group">
                  <input
                     type="name"
                     class="form-control"
                     placeholder="Enter app name"
                     v-model="confirmedAppName"
                     :disabled="noInteraction ? true : false"
                     @keyup="keyPressed">
               </div>

               <div class="alert alert-danger" role="alert" v-if="errorStatusText">
                  {{errorStatusText}} <span v-if="errorStatus" class="badge badge-danger">{{errorStatus}}</span>
               </div>
            </div>
            <div class="modal-footer">
               <button
                  type="button"
                  class="btn btn-danger"
                  @click="deleteApp()"
                  :disabled="(noInteraction || confirmedAppName !== appName) ? true : false">
                  Confirm
               </button>
            </div>
         </div>
      </div>
   </div>
</template>

<script>
   export default {
      data() {
         return {
            errorStatus: null,
            errorStatusText: null,
            confirmedAppName: '',
            noInteraction: false
         }
      },
      props: {
         appName: { type: String }
      },
      methods: {
         open() {
            $( this.$refs.dialog ).modal( {
               backdrop: 'static',
               keyboard: false
            } );
         },
         keyPressed( e ) {
            if ( e.keyCode === 13 ) {
               this.deleteApp();
            }
         },
         deleteApp() {
            if ( this.confirmedAppName !== this.appName ) {
               return;
            }

            this.errorStatus = null;
            this.errorStatusText = null;
            this.noInteraction = true;

            this.$http.delete( '/api/apps/' + this.appName )
               .then( r => {
                  this.noInteraction = false;
                  $( this.$refs.dialog ).modal( 'hide' );

                  // TODO: use a vue.js event to refetch all apps
                  location.reload();
               }, err => {
                  this.noInteraction = false;
                  this.errorStatus = err.status;
                  this.errorStatusText = err.statusText;
               } );
         }
      }
   }
</script>
