/// <reference types="vitest" />
/// <reference types="vite/client" />

import { defineConfig } from "vite";
import { preview } from "@vitest/browser-preview"
import solidPlugin from "vite-plugin-solid";
import devtools from "solid-devtools/vite";

export default defineConfig({
  plugins: [devtools(), solidPlugin()],
  server: {
    port: 3000,
  },
  build: {
    target: "esnext"
  },
  resolve: {
    conditions: ["development", "browser"],
  },
});
