import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import path from "node:path";
import { defineConfig } from "vite";

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  // Prevent Vite from obscuring Rust errors
  clearScreen: false,
  server: {
    // Port comes from TT_DEV_PORT (scripts/dev-port.mjs resolves/pins it and
    // passes it through; also settable directly for a bare `vite` run).
    port: Number(process.env.TT_DEV_PORT) || 1420,
    strictPort: true,
  },
  // Env variables starting with these prefixes are exposed to the client
  envPrefix: ["VITE_", "TAURI_"],
});
