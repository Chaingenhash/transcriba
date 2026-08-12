import { defineConfig } from "vite";

// Tauri expects a fixed port and must not have vite pick another one silently.
// es2022 (not es2021) because main.ts uses top-level await.
export default defineConfig({
  server: { port: 5173, strictPort: true },
  build: { target: "es2022", emptyOutDir: true },
});
