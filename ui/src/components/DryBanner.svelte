<!--
  Файл: ui/src/components/DryBanner.svelte
  Что делает: плашка сухого режима (решение 74) по ответу ядра GET /api/dry (lib/dry.svelte.js). В шапке (always = false) —
    только когда режим сухой: «СУХОЙ РЕЖИМ — ордера и переводы не выполняются» с бюджетом, «не задан dry.budget_usdt»
    или «значение dry.enabled не читается». На «Живом экране» (always = true) — всегда: ещё «обычный режим» или
    «сухой режим не задан — выключен». Ссылка — на категорию настроек «Сигналы и эпизоды».
  Кто вызывает: App.svelte (шапка), pages/Live.svelte.
  От чего зависит: lib/dry.svelte.js.
  Настройки: dry.enabled, dry.budget_usdt (показ).
-->
<script>
  import { dry } from '../lib/dry.svelte.js';

  let { always = false } = $props();
  const d = $derived(dry.data);
  const link = '#/settings/' + encodeURIComponent('Сигналы и эпизоды');
</script>

{#if d?.state === 'on'}
  <div class="banner dry" role="status">
    <strong>СУХОЙ РЕЖИМ</strong> — сигналы без денег: виртуальный бюджет {d.budget} USDT (лимиты money.* действуют);
    ордера, переводы и перегон USDT не выполняются. Начатые сделки доводятся. <a href={link}>Настройки</a>
  </div>
{:else if d?.state === 'nobudget'}
  <div class="banner dry" role="status">
    <strong>СУХОЙ РЕЖИМ</strong> — {d.reason}: сигналов нет, ничего не исполняется. <a href={link}>Задать</a>
  </div>
{:else if d?.state === 'broken'}
  <div class="banner dry" role="status">
    <strong>СУХОЙ РЕЖИМ</strong> ({d.reason}) — ничего не исполняется. <a href={link}>Настройки</a>
  </div>
{:else if always && d?.state === 'off'}
  <div class="banner muted" role="status">Обычный режим: деньги настоящие (dry.enabled = нет). <a href={link}>Настройки</a></div>
{:else if always && d?.state === 'unset'}
  <div class="banner muted" role="status">Сухой режим не задан (dry.enabled) — выключен, обычный режим. <a href={link}>Настройки</a></div>
{:else if always}
  <div class="banner muted" role="status">Режим неизвестен{dry.error ? `: ${dry.error}` : ' — загружаю…'}</div>
{/if}

<style>
  .dry { background: #b45309; color: #fff; font-size: 16px; }
  .dry a { color: #fff; text-decoration: underline; }
</style>
