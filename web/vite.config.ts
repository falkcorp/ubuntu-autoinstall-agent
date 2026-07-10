// file: web/vite.config.ts
// version: 1.0.0
// guid: 0e65c23b-f34d-48b6-9d69-f95345be6191
// last-edited: 2026-07-10

import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Dev-only proxy: CT-07 (constellation control-plane operator API) is not
// implemented yet. Once it lands it will serve /api and /auth on :8443.
// This proxy lets `npm run dev` work against a locally running backend
// without any code changes on either side.
export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      "/api": {
        target: "http://localhost:8443",
        changeOrigin: true,
      },
      "/auth": {
        target: "http://localhost:8443",
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
});
