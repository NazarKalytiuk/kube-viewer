/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
import tailwindcss from "@tailwindcss/vite";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [solid(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 1430,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1431 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "safari16",
    sourcemap: false,
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.{ts,tsx}", "scripts/**/*.test.mjs"],
    server: { deps: { inline: [/solid-js/, /@solidjs/] } },
  },
  resolve: {
    conditions: ["development", "browser"],
  },
});
