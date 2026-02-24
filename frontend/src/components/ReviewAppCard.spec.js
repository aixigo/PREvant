/* @vitest-environment jsdom */
import { describe, it, expect, beforeEach } from "vitest";
import { shallowMount } from "@vue/test-utils";
import { nextTick } from "vue";
import ReviewAppCard from "./ReviewAppCard.vue";

function createReviewApp(containerOverrides = {}, appOverrides = {}) {
  return {
    name: "my-preview",
    status: "deployed",
    owners: [],
    containers: [
      {
        name: "my-preview-service",
        type: "instance",
        status: "running",
        url: "http://localhost:9001/my-preview/my-preview-service/",
        openApiUrl: null,
        asyncApiUrl: null,
        version: null,
        ...containerOverrides,
      },
    ],
    ...appOverrides,
  };
}

function mountCard(reviewApp) {
  const RouterLinkStub = {
    template: "<a><slot /></a>",
  };

  return shallowMount(ReviewAppCard, {
    props: {
      reviewApp,
      showOwners: false,
    },
    global: {
      stubs: {
        "font-awesome-icon": true,
        "router-link": RouterLinkStub,
      },
    },
  });
}

function expectContainerToBeExpandable(wrapper) {
  expect(wrapper.find(".ra-container").classes()).toContain(
    "ra-container__expandable",
  );
}

function expectContainerNotToBeExpandable(wrapper) {
  expect(wrapper.find(".ra-container").classes()).not.toContain(
    "ra-container__expandable",
  );
}

function expectContainerNameAsLink(wrapper) {
  const serviceLink = wrapper.find(".ra-container__infos h5 a");
  expect(serviceLink.exists()).toBe(true);
  expect(serviceLink.text()).toBe("my-preview-service");
}

function expectContainerNameAsText(wrapper) {
  expect(wrapper.find(".ra-container__infos h5 a").exists()).toBe(false);
  expect(wrapper.find(".ra-container__infos h5 span").text()).toBe(
    "my-preview-service",
  );
}

describe("ReviewAppCard", () => {
  describe("container expansion", () => {
    it("marks containers as expandable when version data exists", async () => {
      const wrapper = mountCard(
        createReviewApp({
          status: "paused",
          version: {
            gitCommit: "abcdef1",
            dateModified: "2026-02-20T10:00:00Z",
          },
        }),
      );
      await nextTick();

      expectContainerToBeExpandable(wrapper);
    });

    it("marks containers as expandable when openApiUrl exists", async () => {
      const wrapper = mountCard(
        createReviewApp({
          status: "paused",
          openApiUrl: "/openapi.yml",
          version: null,
          asyncApiUrl: null,
        }),
      );
      await nextTick();

      expectContainerToBeExpandable(wrapper);
    });

    it("marks containers as expandable when asyncApiUrl exists", async () => {
      const wrapper = mountCard(
        createReviewApp({
          status: "paused",
          asyncApiUrl: "/asyncapi.yml",
          version: null,
          openApiUrl: null,
        }),
      );
      await nextTick();

      expectContainerToBeExpandable(wrapper);
    });

    it("marks containers as expandable when container is running", async () => {
      // because in this case the logs will be available so there is something in
      // the expansion area

      const wrapper = mountCard(
        createReviewApp({
          status: "running",
          version: null,
          openApiUrl: null,
          asyncApiUrl: null,
        }),
      );
      await nextTick();

      expectContainerToBeExpandable(wrapper);
    });

    it("does not mark paused containers as expandable when they have no docs and no version", async () => {
      const wrapper = mountCard(
        createReviewApp({
          status: "paused",
          version: null,
          openApiUrl: null,
          asyncApiUrl: null,
        }),
      );
      await nextTick();

      expectContainerNotToBeExpandable(wrapper);
    });

    it("does not mark running containers as expandable when app is backed up", async () => {
      const wrapper = mountCard(
        createReviewApp(
          {
            status: "running",
            version: null,
            openApiUrl: null,
            asyncApiUrl: null,
          },
          { status: "backed-up" },
        ),
      );
      await nextTick();

      expectContainerNotToBeExpandable(wrapper);
    });

    it("does not toggle expansion state for non-expandable containers", async () => {
      const wrapper = mountCard(
        createReviewApp({
          status: "paused",
          version: null,
          openApiUrl: null,
          asyncApiUrl: null,
        }),
      );
      await nextTick();

      const containerType = wrapper.find(".ra-container__type");
      expect(containerType.classes()).not.toContain("is-expanded");
      expect(wrapper.text()).not.toContain("Logs");

      await containerType.trigger("click");
      await nextTick();

      expect(containerType.classes()).not.toContain("is-expanded");
      expect(wrapper.text()).not.toContain("Logs");
    });
  });

  describe("logs link", () => {
    it("renders logs link for running containers", async () => {
      const wrapper = mountCard(createReviewApp({ status: "running" }));
      await nextTick();

      expect(wrapper.text()).toContain("Logs");
    });

    it("does not render logs link for paused containers", async () => {
      const wrapper = mountCard(createReviewApp({ status: "paused" }));
      await nextTick();

      expect(wrapper.text()).not.toContain("Logs");
    });

    it("does not render logs link when app is backed up", async () => {
      const wrapper = mountCard(
        createReviewApp({ status: "running" }, { status: "backed-up" }),
      );
      await nextTick();

      expect(wrapper.text()).not.toContain("Logs");
    });
  });

  describe("container link", () => {
    it("renders container name as link when URL exists and container is running", async () => {
      const wrapper = mountCard(
        createReviewApp({
          status: "running",
          url: "http://localhost:9001/my-preview/my-preview-service/",
        }),
      );
      await nextTick();

      expectContainerNameAsLink(wrapper);
    });

    it("does not render container name as link when container is paused", async () => {
      const wrapper = mountCard(createReviewApp({ status: "paused" }));
      await nextTick();

      expectContainerNameAsText(wrapper);
    });

    it("does not render container name as link when app is backed up", async () => {
      const wrapper = mountCard(
        createReviewApp({ status: "running" }, { status: "backed-up" }),
      );
      await nextTick();

      expectContainerNameAsText(wrapper);
    });

    it("does not render container name as link when URL is missing", async () => {
      const wrapper = mountCard(
        createReviewApp({ status: "running", url: null }),
      );
      await nextTick();

      expectContainerNameAsText(wrapper);
    });
  });
});
