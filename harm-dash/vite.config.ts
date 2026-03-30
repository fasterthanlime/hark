import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      "/api": {
        target: "http://souffle.dropbear-piranha.ts.net:3456",
        changeOrigin: true,
      },
    },
  },
});
