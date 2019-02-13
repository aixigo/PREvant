<template>
   <transition name="modal">
      <div class="modal-mask">
         <div class="modal-wrapper">
            <div class="modal-container">

               <div class="modal-header">
                  <h1>API Documentation<span v-if="showAdditionalHeadlineInformation"> – {{ this.title }}</span></h1>

                  <font-awesome-icon icon="window-close" @click="close()" />
               </div>

               <div class="modal-body">
                  <div :id="`swagger-ui-${this._uid}`"></div>
               </div>
            </div>
         </div>
      </div>
   </transition>
</template>

<style type="text/css">
   @import "~swagger-ui-dist/swagger-ui.css";

   .modal-mask {
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

   .modal-wrapper {
      display: table-cell;
      vertical-align: middle;
   }

   .modal-container {
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

   .modal-header h3 {
      margin-top: 0;
      color: #42b983;
   }

   .modal-body {
      margin: 20px 0;
   }

   .modal-enter .modal-container,
   .modal-leave-active .modal-container {
      -webkit-transform: scale(0.1);
      transform: scale(0.1);
   }
</style>

<script>
   const SwaggerUIBundle = require( 'swagger-ui-dist' ).SwaggerUIBundle;

   export default {
      data() {
         return {};
      },
      props: {
         url: {
            type: String
         },
         title: {
            type: String,
            default: null
         }
      },
      computed: {
         showAdditionalHeadlineInformation() {
            return this.title != null;
         }
      },
      mounted() {
         const ui = SwaggerUIBundle( {
            url: this.url,
            dom_id: `#swagger-ui-${this._uid}`,
            presets: [
               SwaggerUIBundle.presets.apis,
               SwaggerUIBundle.SwaggerUIStandalonePreset
            ]
         } );
      },
      methods: {
         close() {
            this.$emit('close');
         }
      }
   }
</script>