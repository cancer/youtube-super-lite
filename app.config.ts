import { defineConfig } from "@solidjs/start/config";
import UnoCSS from "unocss/vite";
import Icons from "unplugin-icons/vite";

export default defineConfig({
  server: {
    preset: "cloudflare-pages",
    rollupConfig: {
      external: ["__STATIC_CONTENT_MANIFEST", "node:async_hooks"],
    },
  },
  middleware: "./src/middlewares/index.ts",
  vite: {
    plugins: [UnoCSS(), Icons({ compiler: "solid" })],
  },
});
