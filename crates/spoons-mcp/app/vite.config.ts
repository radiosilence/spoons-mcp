// One self-contained HTML file, scripts and styles inlined, which the server embeds and serves as the ui:// resource.
import { defineConfig } from "vite";
import { viteSingleFile } from "vite-plugin-singlefile";

export default defineConfig({
  esbuild: { jsx: "automatic", jsxImportSource: "preact" },
  plugins: [viteSingleFile({ removeViteModuleLoader: true })],
  build: {
    outDir: "../assets",
    emptyOutDir: false,
    target: "es2022",
    reportCompressedSize: true,
    rollupOptions: { input: "app.html" },
  },
});
