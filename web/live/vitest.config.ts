import { defineConfig } from "vitest/config";
import solidPlugin from "vite-plugin-solid";
import { preview } from "@vitest/browser-preview";

export default defineConfig({
  plugins: [solidPlugin()],
  resolve: {
    conditions: ["development", "browser"],
  },
  test: {
    environment: "jsdom",
    browser: {
      enabled: true,
      provider: preview(),
      instances: [{ browser: "chromium" }],
    },
    setupFiles: "./vitest.setup.ts",
  },
});
