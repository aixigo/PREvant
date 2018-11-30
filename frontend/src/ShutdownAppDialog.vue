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