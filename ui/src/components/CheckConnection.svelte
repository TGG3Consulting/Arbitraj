<!--
  Файл: ui/src/components/CheckConnection.svelte
  Что делает: кнопка «Проверить подключение». Ответ бэкенда {ok, details}:
    ok — «Подключено …» зелёным; иначе точная причина сбоя красным (текст бэкенда как есть).
  Кто вызывает: pages/Notifications.svelte (Telegram), pages/Venues.svelte (площадки).
  От чего зависит: lib/api.js; бэкенд src/api/check.rs.
  Настройки: нет.
-->
<script>
  import { post } from '../lib/api.js';

  /** url — адрес проверки, например /api/check/telegram. */
  let { url } = $props();
  let busy = $state(false);
  let result = $state(null);

  /** Отправляет проверку и показывает результат; сбой самого запроса — тоже красным. */
  async function check() {
    busy = true;
    result = null;
    try {
      result = await post(url);
    } catch (e) {
      result = { ok: false, details: e.message };
    }
    busy = false;
  }
</script>

<div class="check">
  <button onclick={check} disabled={busy}>{busy ? 'Проверяю…' : 'Проверить подключение'}</button>
  {#if result}
    <span class={result.ok ? 'ok' : 'err'} role="status">{result.details}</span>
  {/if}
</div>
