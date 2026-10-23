// Файл: ui/vite.config.js
// Что делает: сборка интерфейса (npm run build → ui/dist) и сервер разработки с proxy на бэкенд.
// Кто вызывает: npm run dev / npm run build.
// От чего зависит: vite, @sveltejs/vite-plugin-svelte.
// Настройки: адрес бэкенда для разработки — переменная окружения ARB_API (например http://127.0.0.1:8080).
//   Значения по умолчанию нет: без ARB_API сервер разработки не запускается (сборке она не нужна).

import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig(({ command }) => {
  const config = { plugins: [svelte()], build: { outDir: 'dist', emptyOutDir: true } };
  if (command !== 'serve') return config;

  const target = process.env.ARB_API;
  if (!target) {
    throw new Error('Не задан ARB_API — адрес бэкенда, например ARB_API=http://127.0.0.1:8080 npm run dev');
  }
  // changeOrigin: Host запроса = адрес бэкенда (его проверяет api/guard.rs).
  // Origin браузера (адрес vite) передаётся как есть — бэкенд запускается с --allow-origin <адрес vite>.
  config.server = { proxy: { '/api': { target, changeOrigin: true, ws: true } } };
  return config;
});
