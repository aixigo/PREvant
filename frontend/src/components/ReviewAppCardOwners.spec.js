/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import { mount } from "@vue/test-utils";

vi.mock("mdb-vue-ui-kit", () => {
  return {
    MDBBadge: {
      template: '<span class="mdb-badge"><slot /></span>',
    },
    MDBTooltip: {
      template:
        '<span class="mdb-tooltip"><slot name="reference" /><slot name="tip" /></span>',
    },
  };
});

import ReviewAppCardOwners from "./ReviewAppCardOwners.vue";

function mountOwners(owners) {
  return mount(ReviewAppCardOwners, {
    props: { owners },
    global: {
      renderStubDefaultSlot: true,
    },
  });
}

describe("ReviewAppCardOwners", () => {
  it("shows fallback text for empty owners", () => {
    const wrapper = mountOwners([]);

    expect(wrapper.text()).toContain("No known owners");
    expect(wrapper.text()).not.toContain("Owners:");
  });

  it("shows first owner name when one owner exists", () => {
    const wrapper = mountOwners([{ name: "Max Mustermann" }]);

    expect(wrapper.text()).toContain("Owners:");
    expect(wrapper.text()).toContain("Max Mustermann");
    expect(wrapper.text()).not.toContain("+1");
    expect(wrapper.findAll(".mdb-badge")).toHaveLength(0);
  });

  it("falls back to owner sub if name is missing", () => {
    const wrapper = mountOwners([{ sub: "mmustermann" }]);

    expect(wrapper.text()).toContain("Owners:");
    expect(wrapper.text()).toContain("mmustermann");
  });

  it("shows additional owners count as +X", () => {
    const wrapper = mountOwners([
      { name: "Max Mustermann" },
      { name: "Jane Doe" },
      { sub: "jdoe" },
    ]);

    expect(wrapper.text()).toContain("Max Mustermann");
    expect(wrapper.text()).toContain("+2");
    expect(wrapper.text()).toContain("Jane Doe");
    expect(wrapper.text()).toContain("jdoe");
  });
});
