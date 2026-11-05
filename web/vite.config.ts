import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";
const host = "127.0.0.1";
const apiTarget = `http://${host}:8080`;
export default defineConfig({
  plugins: [react()],
  server: {
    host: "localhost",
    proxy: {
      "/api": apiTarget,
      "/agent": { target: apiTarget, ws: true },
    },
  },
  build: {
    sourcemap: false,
    rolldownOptions: {
      output: {
        codeSplitting: true,
      },
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test-setup.ts"],
    restoreMocks: true,
  },
});
