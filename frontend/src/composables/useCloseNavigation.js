import { useRoute, useRouter } from "vue-router";

/**
 * Provides a single close-navigation behavior for modal-like routes.
 *
 * When a dialog view is closed, we prefer navigating to the router history
 * `back` target. If no `back` target exists or if it resolves to the current
 * route (for example after direct URL entry), we fall back to `/` to ensure
 * closing always results in an actual navigation.
 *
 * A missing `back` target typically happens when the route was opened directly
 * (deep link/bookmark/new tab/refresh) instead of being reached via in-app
 * navigation.
 */
export function useCloseNavigation() {
  const router = useRouter();
  const route = useRoute();

  function resolveCloseTarget(router, currentRoute) {
    const back = router.options.history.state.back;
    if (!back) {
      return "/";
    }

    const normalizedBack = back.split("#").at(-1);
    return normalizedBack === currentRoute ? "/" : back;
  }

  function navigateOnClose() {
    router.push(resolveCloseTarget(router, route.fullPath));
  }

  return {
    navigateOnClose,
  };
}
