import { computed } from "vue";
import { useConfig } from "./useConfig";

export function useAuth() {
  const issuers = computed(() => window.issuers ?? null);
  const me = computed(() => window.me ?? null);

  const { isAuthRequired } = useConfig();

  const hasWritePermissions = computed(() => {
    if( !isAuthRequired.value ) {
      return true;
    }

    return me.value != null;
  });

  return {
    issuers,
    me,
    isAuthRequired,
    hasWritePermissions,

  };
}
