import { beforeEach, describe, expect, it, vi } from "vitest";
import { useCloseNavigation } from "./useCloseNavigation";
import { useRoute, useRouter } from "vue-router";

vi.mock("vue-router", () => ({
  useRoute: vi.fn(),
  useRouter: vi.fn(),
}));

describe("useCloseNavigation", () => {
  const push = vi.fn();

  beforeEach(() => {
    useRoute.mockReturnValue({ fullPath: "/logs/my-preview/whoami" });
    useRouter.mockReturnValue({
      options: { history: { state: { back: "/#/" } } },
      push,
    });
  });

  it("should navigate to back target when it differs from current route", () => {
    useCloseNavigation().navigateOnClose();
    expect(push).toHaveBeenCalledWith("/#/");
  });

  it("should fall back to root when no back target exists", () => {
    useRouter.mockReturnValue({
      options: { history: { state: {} } },
      push,
    });

    useCloseNavigation().navigateOnClose();

    expect(push).toHaveBeenCalledWith("/");
  });

  it("should fall back to root when back target resolves to current route", () => {
    useRouter.mockReturnValue({
      options: {
        history: {
          state: { back: "http://localhost:9001/#/logs/my-preview/whoami" },
        },
      },
      push,
    });

    useCloseNavigation().navigateOnClose();

    expect(push).toHaveBeenCalledWith("/");
  });
});
