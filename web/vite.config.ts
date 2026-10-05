import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Built into the static website: https://<site>/konto/, next to the pages in
// website/. The API is the same origin under /api (Caddy forwards it).
export default defineConfig({
  base: "/konto/",
  plugins: [svelte()],
  build: { target: "es2022", outDir: "../website/konto", emptyOutDir: true },
  server: { proxy: { "/api": "http://127.0.0.1:21380" } },
});
