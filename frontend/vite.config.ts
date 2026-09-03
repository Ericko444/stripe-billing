/// <reference types="vitest/config" />
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// D1: the only origin the browser ever talks to is this dev server.
// /api proxies straight through to `demo`, so no CORS layer is needed in
// the module -- CORS policy is host policy, and this keeps it out.
export default defineConfig({
  plugins: [react()],
  // Vite loads `.env` from its own project root by default; the real one
  // (VITE_STRIPE_PUBLISHABLE_KEY, and everything `dotenvy` reads for
  // `demo`) lives at the repo root, one directory up. Pointing here rather
  // than duplicating a second `frontend/.env` keeps one source of truth.
  envDir: "..",
  server: {
    port: 5173,
    proxy: {
      "/api": {
        target: "http://localhost:8080",
        changeOrigin: true,
      },
    },
  },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: "./src/test-setup.ts",
  },
});
