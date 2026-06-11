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
        AsyncAPI Documentation
      </MDBModalTitle>
    </MDBModalHeader>

    <MDBModalBody>
      <div ref="asyncapi"></div>
    </MDBModalBody>
  </MDBModal>
</template>

<style src="@asyncapi/react-component/styles/default.min.css"></style>

<script setup>
import { ref, onMounted, useTemplateRef } from "vue";
import { useRoute } from "vue-router";
import AsyncApiStandalone from "@asyncapi/react-component/browser/standalone";
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
const asyncapi = useTemplateRef("asyncapi");
function renderOpenApi() {
  AsyncApiStandalone.render(
    {
      schema: { url: route.params.url },
      config: {},
    },
    asyncapi.value
  );
}

function handleClose() {
  navigateOnClose();
}
</script>
