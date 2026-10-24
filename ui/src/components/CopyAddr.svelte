<!--
  Файл: ui/src/components/CopyAddr.svelte
  Что делает: полный адрес контракта и кнопка «Копировать» (Issue #33); после копирования — «скопировано»
    или причина, если браузер не дал доступ к буферу обмена.
  Кто вызывает: components/LiveCoinModal.svelte, components/CoinModal.svelte.
  От чего зависит: ничего (navigator.clipboard браузера).
  Настройки: нет.
-->
<script>
  /** addr — адрес (строка) или null — биржа не сообщила адрес. */
  let { addr } = $props();
  let note = $state('');

  /** Копирует адрес в буфер обмена. Ошибка браузера — текстом рядом. */
  async function copy() {
    try {
      await navigator.clipboard.writeText(addr);
      note = 'скопировано';
    } catch (e) {
      note = `не скопировано: ${e.message}`;
    }
  }
</script>

<div class="row">
  {#if addr}
    <code class="addr">{addr}</code>
    <button class="quiet" onclick={copy}>Копировать</button>
    {#if note}<span class="muted" role="status">{note}</span>{/if}
  {:else}
    <span class="muted">адрес контракта не сообщён</span>
  {/if}
</div>

<style>
  .addr { font-size: 14px; user-select: all; }
</style>
