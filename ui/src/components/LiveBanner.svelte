<!--
  Файл: ui/src/components/LiveBanner.svelte
  Что делает: плашка «Нет живой связи с сервером» с причиной и кнопкой «Переподключиться».
    Пока связи нет, изменения из других вкладок и новые записи журнала не приходят.
  Кто вызывает: App.svelte.
  От чего зависит: lib/live.svelte.js.
  Настройки: нет.
-->
<script>
  import { live, connect } from '../lib/live.svelte.js';
</script>

{#if !live.connected}
  <div class="banner warn" role="status">
    Нет живой связи с сервером{live.reason ? `: ${live.reason}` : ''}. Изменения из других вкладок не видны.
    <button onclick={connect}>Переподключиться</button>
  </div>
{/if}
