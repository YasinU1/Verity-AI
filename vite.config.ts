import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed dev port and does not want the CLI to obscure Rust errors.
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],
  // Prevent Vite from clearing the screen so Rust compiler errors stay visible.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? { protocol: "ws", host, port: 1421 }
      : undefined,
    watch: {
      // Don't watch the Rust source tree from the JS dev server.
      ignored: ["**/src-tauri/**"],
    },
  },
  // Multi-window: one bundle, routed by URL hash (#main / #overlay).
  build: {
    target: "es2021",
    minify: process.env.TAURI_DEBUG ? false : "esbuild",
    sourcemap: !!process.env.TAURI_DEBUG,
  },
});
