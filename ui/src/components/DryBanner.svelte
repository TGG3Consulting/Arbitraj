<!--
  Файл: ui/src/components/DryBanner.svelte
  Что делает: плашка сухого режима (решение 74). В шапке (always = false) — только когда сухой режим включён: заметная
    плашка «СУХОЙ РЕЖИМ — ордера и переводы не выполняются», бюджет или «не задан dry.budget_usdt — сигналов нет».
    На «Живом экране» (always = true) — всегда: ещё «обычный режим — деньги настоящие» или «сухой режим не задан —
    выключен». Ссылка — на категорию настроек «Сигналы и эпизоды».
  Кто вызывает: App.svelte (шапка), pages/Live.svelte.
  От чего зависит: lib/settings.svelte.js, lib/dry.js.
  Настройки: dry.enabled, dry.budget_usdt (показ).
-->
<script>
  import { store } from '../lib/settings.svelte.js';
  import { dryState } from '../lib/dry.js';

  let { always = false } = $props();
  const d = $derived(dryState(store.values));
  const link = '#/settings/' + encodeURIComponent('Сигналы и эпизоды');
</script>

{#if d.state === 'on'}
  <div class="banner dry" role="status">
    <strong>СУХОЙ РЕЖИМ</strong> — сигналы без денег: виртуальный бюджет {d.budget} USDT (лимиты money.* действуют);
    ордера, переводы и перегон USDT не выполняются. Начатые сделки доводятся. <a href={link}>Настройки</a>
  </div>
{:else if d.state === 'nobudget'}
  <div class="banner dry" role="status">
    <strong>СУХОЙ РЕЖИМ</strong> — не задан dry.budget_usdt: сигналов нет, ничего не исполняется.
    <a href={link}>Задать</a>
  </div>
{:else if always && d.state === 'off'}
  <div class="banner muted" role="status">Обычный режим: деньги настоящие (dry.enabled = нет). <a href={link}>Настройки</a></div>
{:else if always}
  <div class="banner muted" role="status">Сухой режим не задан (dry.enabled) — выключен, обычный режим. <a href={link}>Настройки</a></div>
{/if}

<style>
  .dry { background: #b45309; color: #fff; font-size: 16px; }
  .dry a { color: #fff; text-decoration: underline; }
</style>
