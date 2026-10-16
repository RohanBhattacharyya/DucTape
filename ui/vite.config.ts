import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { resolve } from "node:path";

// For `npm run dev`, run the backend with a fixed address:
//   cargo run -- --headless --ui-addr 127.0.0.1:7340
const backend = process.env.CONDUCTOR_BACKEND ?? "127.0.0.1:7340";

export default defineConfig({
  plugins: [react()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
    rolldownOptions: {
      input: {
        index: resolve(import.meta.dirname, "index.html"),
        console: resolve(import.meta.dirname, "console.html"),
      },
    },
  },
  server: {
    proxy: {
      "/ws": { target: `ws://${backend}`, ws: true },
      "/api": `http://${backend}`,
    },
  },
});
