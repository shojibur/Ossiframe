import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const here = (p: string) => fileURLToPath(new URL(p, import.meta.url));

// Tauri expects a fixed port and must see Rust errors in the terminal.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  resolve: {
    // Module UI folders live outside this package, so they cannot find
    // node_modules on their own. Point them at the same copies Doctor uses.
    alias: {
      "@ossiframe/ui": here("../../packages/ui/src/index.ts"),
      "@ossiframe/client": here("../../packages/client/src/index.ts"),
    },
    dedupe: ["react", "react-dom"],
  },
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**"] } },
  build: { target: "safari16", sourcemap: false },
});
