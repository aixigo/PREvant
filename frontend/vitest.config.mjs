import { defineConfig, mergeConfig } from "vitest/config";
import viteConfig from "./vite.config.mjs";

export default mergeConfig(
  viteConfig,
  defineConfig({
    test: {
      include: ["src/**/*.spec.?(c|m)[jt]s?(x)"],
      reporters: ["default", "junit"],
      outputFile: {
        junit: "reports/unit/junit.xml",
      },
      coverage: {
        provider: "v8",
        reportsDirectory: "reports/coverage",
        reporter: ["text", "html", "cobertura"],
        all: true,
        include: ["**/*.{js,vue}"],
        exclude: ["**/tests/**", "**/node_modules/**"],
      },
    },
  })
);
