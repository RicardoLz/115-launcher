import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
// @ts-expect-error type error without @types/node package
import { fileURLToPath } from "node:url";

const mock = (name: string) => fileURLToPath(new URL(`./sandbox/tauri/${name}`, import.meta.url));

/// 沙盒专用：只把 @tauri-apps/api 的三个模块换成假实现，其余（App.tsx、bridge.ts、
/// styles.css、所有组件）用的都是真文件。生产构建走 vite.config.ts，不碰这里。
export default defineConfig(() => ({
  plugins: [react()],
  clearScreen: false,
  resolve: {
    alias: [
      { find: /^@tauri-apps\/api\/core$/, replacement: mock("core.ts") },
      { find: /^@tauri-apps\/api\/event$/, replacement: mock("event.ts") },
      { find: /^@tauri-apps\/api\/window$/, replacement: mock("window.ts") },
    ],
  },
  server: { port: 1430, strictPort: true },
}));
