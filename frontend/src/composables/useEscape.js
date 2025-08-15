import { onMounted, onUnmounted } from "vue";

export function useEscape(callback) {
  function handleKeydown(event) {
    if (event.key === "Escape") {
      callback();
    }
  }

  onMounted(() => {
    window.addEventListener("keydown", handleKeydown);
  });

  onUnmounted(() => {
    window.removeEventListener("keydown", handleKeydown);
  });
}
