import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  // Relative, so the built site works from a domain root, a GitHub Pages project
  // subpath (/Vex/) or a file:// preview without being rebuilt for each.
  base: "./",
  server: {
    // The screenshots are imported from `docs/images`, which is outside this
    // root. One copy of a PNG in the repository, not two.
    fs: { allow: [".."] },
  },
});
