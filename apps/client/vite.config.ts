import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import path from "node:path";
import { defineConfig } from "vite";
import { slotBasePort } from "../../scripts/slot-port.mjs";

// `dev-port.mjs` normally pins TT_DEV_PORT before launching us. If vite is run
// directly (no TT_DEV_PORT), fall back to this slot's deterministic base port
// rather than a hardcoded 1420, so slots don't squat on each other's port.
const devPort = Number(process.env.TT_DEV_PORT) || slotBasePort(path.resolve(__dirname, "../.."));

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  build: {
    rollupOptions: {
      // Two entries: the main app and the standalone Settings window.
      input: {
        main: path.resolve(__dirname, "index.html"),
        settings: path.resolve(__dirname, "settings.html"),
      },
    },
  },
  // Prevent Vite from obscuring Rust errors
  clearScreen: false,
  server: {
    port: devPort,
    strictPort: true,
  },
  // Env variables starting with these prefixes are exposed to the client
  envPrefix: ["VITE_", "TAURI_"],
});
