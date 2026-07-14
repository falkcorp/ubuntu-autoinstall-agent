// file: web/vite.config.ts
// version: 1.1.0
// guid: 0e65c23b-f34d-48b6-9d69-f95345be6191
// last-edited: 2026-07-14

import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Dev-only proxy: lets `npm run dev` work against a locally running backend
// without any code changes on either side. The operator API/SPA plane binds
// :15000 (renumbered 2026-07-14 from :15001, part of a :15000/:15001/:15002
// contiguous block — see crates/uaa-control/src/listeners.rs).
export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      "/api": {
        target: "http://localhost:15000",
        changeOrigin: true,
      },
      "/auth": {
        target: "http://localhost:15000",
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
});
