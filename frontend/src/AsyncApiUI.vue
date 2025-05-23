<template>
   <transition name="modal">
      <div class="ra-modal-mask">
         <div class="ra-modal-wrapper">
            <div class="ra-modal-container">

               <div class="ra-modal-header">
                  <h1>AsyncAPI Documentation<span v-if="showAdditionalHeadlineInformation"> – {{ $route.params.title }}</span></h1>

                  <span @click="close()">
                     <font-awesome-icon icon="window-close" @click="close()" class="ra-modal-close-button"/>
                  </span>
               </div>

               <div class="ra-modal-body">
                  <div ref="asyncapi"></div>
               </div>
            </div>
         </div>
      </div>
   </transition>
</template>

<style scope src="@asyncapi/react-component/styles/default.min.css"></style>

<script>
import AsyncApiStandalone from '@asyncapi/react-component/browser/standalone';

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
      const container = this.$refs.asyncapi;
      AsyncApiStandalone.render({
         schema: { url: this.$route.params.url },
         config: {}
      }, container);
   },
   methods: {
      close() {
         this.$router.back();
      }
   }
}
</script>
