<!--
  Файл: ui/src/components/LockBanner.svelte
  Что делает: заметная плашка «Система заблокирована» на всех экранах, пока секреты не разблокированы,
    со ссылкой на экран «Разблокировка».
  Кто вызывает: App.svelte.
  От чего зависит: lib/secrets.svelte.js, lib/router.svelte.js.
  Настройки: нет.
-->
<script>
  import { secrets } from '../lib/secrets.svelte.js';
  import { href } from '../lib/router.svelte.js';
</script>

{#if secrets.status && !secrets.status.unlocked}
  <div class="banner lock" role="alert">
    <strong>Система заблокирована.</strong>
    {secrets.status.file_exists
      ? 'Секреты не разблокированы — ключи площадок и токен Telegram недоступны, торговли нет.'
      : 'Файл секретов ещё не создан — ключи площадок и токен Telegram негде хранить, торговли нет.'}
    <a href={href('unlock')}>{secrets.status.file_exists ? 'Разблокировать' : 'Создать'}</a>
  </div>
{/if}
{#if secrets.error}
  <div class="banner lock" role="alert">Не удалось узнать состояние секретов: {secrets.error}</div>
{/if}
