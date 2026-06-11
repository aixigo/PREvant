<template>
  <MDBModal
    v-model="visible"
    centered
    size="lg"
    @hide="handleClose"
    @show="renderOpenApi"
  >
    <MDBModalHeader>
      <MDBModalTitle>
        API Documentation
      </MDBModalTitle>
    </MDBModalHeader>

    <MDBModalBody>
      <div class="open-api-ui" ref="openapi"></div>
    </MDBModalBody>
  </MDBModal>
</template>

<style lang="css" src="swagger-ui/dist/swagger-ui.css"></style>

<style lang="scss" scoped>
/* Fixes swagger ui response column width */
.open-api-ui {
  :deep(.response-col_status) {
    width: 10% !important;
  }
  :deep(.parameters-col_name) {
    width: 10% !important;
  }
}
</style>

<script setup>
import { ref, onMounted, useTemplateRef } from "vue";
import { useRoute } from "vue-router";
import SwaggerUI from "swagger-ui";
import {
  MDBModal,
  MDBModalHeader,
  MDBModalBody,
  MDBModalTitle,
} from "mdb-vue-ui-kit";
import { useCloseNavigation } from "../composables/useCloseNavigation";

const visible = ref(false);
onMounted(() => {
  visible.value = true;
});

const route = useRoute();
const { navigateOnClose } = useCloseNavigation();
const openapi = useTemplateRef("openapi");

function renderOpenApi() {
  SwaggerUI({
    url: route.params.url,
    domNode: openapi.value,
  });
}

function handleClose() {
  navigateOnClose();
}
</script>
