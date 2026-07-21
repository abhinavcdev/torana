import { defineConfig } from "astro/config";
import sitemap from "@astrojs/sitemap";

export default defineConfig({
  site: "https://abhinavcdev.github.io",
  base: "/torana",
  trailingSlash: "always",
  integrations: [sitemap()],
  compressHTML: true,
});
