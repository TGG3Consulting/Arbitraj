// Проверка интерфейса перед сборкой (решение 212): любое имя без объявления или импорта — ошибка сборки.
// Кто вызывает: `npm run build` (скрипт prebuild-проверки `lint`). Зависит от eslint, svelte-eslint-parser, globals.
// Причина: vite собирает .svelte с неизвестными именами в разметке без ошибки, а в браузере это ReferenceError.
import svelteParser from 'svelte-eslint-parser';
import globals from 'globals';

const runes = { $state: 'readonly', $derived: 'readonly', $effect: 'readonly', $props: 'readonly',
  $bindable: 'readonly', $inspect: 'readonly', $host: 'readonly' };

export default [
  { ignores: ['dist/**', 'node_modules/**'] },
  { files: ['src/**/*.svelte', 'src/**/*.svelte.js'],
    languageOptions: { parser: svelteParser, ecmaVersion: 2024, sourceType: 'module',
      globals: { ...globals.browser, ...runes } },
    rules: { 'no-undef': 'error' } },
  { files: ['src/**/*.js', 'vite.config.js'], ignores: ['src/**/*.svelte.js'],
    languageOptions: { ecmaVersion: 2024, sourceType: 'module', globals: { ...globals.browser, ...globals.node } },
    rules: { 'no-undef': 'error' } },
];
