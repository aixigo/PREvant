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

            <input class="form-control mr-sm-2" type="search" placeholder="Search Review Apps" aria-label="Search"
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
            <a class="btn btn-outline-success my-2 my-sm-0" v-if="portainerUrl !== ''"
               :href="portainerUrl" target="_blank">
               <font-awesome-icon icon="server"/>
               Portainer
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

      computed: {
         ...mapGetters(['swaggerUrl', 'portainerUrl'])
      },

      methods: {
         fireSearchEvent() {
            this.$store.commit( 'filterByAppName', this.$refs.searchApps.value );
         }
      }
   }
</script>
