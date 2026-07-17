import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import path from "node:path";
import { defineConfig } from "vite";
import { resolveDevPort } from "../../scripts/slot-port.mjs";

// `dev-port.mjs` normally pins TT_DEV_PORT before launching us. If vite is run
// directly, resolve the same per-checkout claim from the repo root's rendered
// `.env`/`.env.local`; a checkout with no claim at all gets 1420 (bare-vite
// mock dev only — every tt-managed checkout has a claim, so this never
// collides across slots in practice).
const devPort =
  Number(process.env.TT_DEV_PORT) || resolveDevPort(path.resolve(__dirname, "../..")) || 1420;

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
    port: devPort,
    strictPort: true,
  },
  // Env variables starting with these prefixes are exposed to the client
  envPrefix: ["VITE_", "TAURI_"],
});
