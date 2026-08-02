// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5273,
    strictPort: true,
    // Proxied rather than called cross-origin: the packaged Tauri app talks to
    // the server directly, so the browser dev setup should not be the only
    // configuration that needs CORS on the server.
    proxy: {
      "/v1": {
        target: process.env.FURY_SERVER ?? "http://127.0.0.1:8901",
        changeOrigin: true,
      },
    },
  },
});
