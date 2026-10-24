<!--
  Файл: ui/src/components/Attribution.svelte
  Что делает: подпись источника данных справочника пулов (решение 58), если её требуют условия индексатора:
    текст и ссылка — из настроек index.attribution_text / index.attribution_url. Не задана — ничего.
  Кто вызывает: components/PoolDirectory.svelte, components/CoinDirectory.svelte.
  От чего зависит: ничего.
  Настройки: index.attribution_text, index.attribution_url (через ответ GET /api/venues/{id}/pool-directory).
-->
<script>
  /** text — подпись или null; url — ссылка или null. */
  let { text, url } = $props();
</script>

{#if text}
  <p class="muted attribution">
    {#if url}<a href={url} target="_blank" rel="noreferrer noopener">{text}</a>{:else}{text}{/if}
  </p>
{/if}

<style>
  .attribution { font-size: 0.85em; margin: 4px 0; }
</style>
