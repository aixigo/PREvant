import { computed } from "vue";

export function useConfig() {
  const injectedConfig = computed(() => window.config ?? {});
  const defaultAppName = computed(() => injectedConfig.value.defaultAppName);

  return {
    defaultAppName,
  };
}
