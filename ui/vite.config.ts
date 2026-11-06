import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Relative base so the bundle works from the Tauri asset protocol.
export default defineConfig({ base: "./", plugins: [react()], server: { port: 5173, strictPort: true } });
