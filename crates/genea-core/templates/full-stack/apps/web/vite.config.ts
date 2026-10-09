import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  server: {
    // The API (`apps/api`) listens on port 3000: one URL, no CORS.
    proxy: { "/api": "http://localhost:3000" },
  },
  test: {
    environment: "node",
  },
});
