<!--
  Файл: ui/src/components/WalletMap.svelte
  Что делает: вкладка «Маппинг» экрана «Маппинг кошельков» (решение 132): строка на каждую биржу реестра шлюза (список
    — от бэкенда, без перечисления здесь), строка «DEX ↔ DEX (циклы)» и «Пробы и сигналы» (симуляции налогов,
    крючков, оценки разрешений — без денег) → выпадающий список кошельков (PUT /api/wallets/map {row, wallet}).
    Биржа без кошелька — с DEX не торгует (подпись). Один кошелёк на нескольких денежных строках — явное предупреждение
    «деньги смешиваются». История «было/стало» (GET /api/wallets/history?limit=N, N задаёт зритель — PageSize).
  Кто вызывает: pages/Wallets.svelte.
  От чего зависит: lib/api.js, components/{PageSize,ErrorText}.svelte, lib/labels.js (showTime).
  Настройки: нет.
-->
<script>
  import { uniqBy } from '../lib/uniq.js';
  import { get, put, seg } from '../lib/api.js';
  import { showTime } from '../lib/labels.js';
  import PageSize from './PageSize.svelte';
  import ErrorText from './ErrorText.svelte';

  /** view — ответ GET /api/wallets; onchange — перечитать экран. */
  let { view, onchange } = $props();

  let error = $state('');
  let limit = $state(null);
  let history = $state([]);

  const names = $derived(Object.fromEntries(view.wallets.map((w) => [w.id, w.name])));

  /** Подпись строки маппинга. */
  function title(r) {
    if (r.kind === 'dex_dex') return 'DEX ↔ DEX (циклы)';
    if (r.kind === 'probe') return 'Пробы и сигналы (без денег)';
    return `${r.row} ↔ DEX`;
  }

  /** Привязывает строку к кошельку ('' — отвязать). */
  async function bind(row, wallet) {
    error = '';
    try {
      await put('/api/wallets/map', { row, wallet: wallet || null });
      onchange?.();
    } catch (e) {
      error = e.message;
    }
  }

  /** История изменений (N — от зрителя). */
  async function loadHistory() {
    if (!limit) return;
    try {
      history = await get(`/api/wallets/history?limit=${seg(String(limit))}`);
    } catch (e) {
      error = e.message;
    }
  }

  $effect(() => {
    void view;
    loadHistory();
  });
</script>

{#each uniqBy(view.shared, (s) => s.wallet) as s (s.wallet)}
  <p class="err" role="alert">Деньги смешиваются: кошелёк «{s.name}» привязан к {s.rows.join(', ')}.</p>
{/each}

<table>
  <thead><tr><th>Строка</th><th>Кошелёк</th><th></th></tr></thead>
  <tbody>
    {#each uniqBy(view.rows, (r) => r.row) as r (r.row)}
      <tr>
        <td>{title(r)}</td>
        <td>
          <select value={r.wallet ?? ''} onchange={(e) => bind(r.row, e.currentTarget.value)}>
            <option value="">— нет кошелька —</option>
            {#each uniqBy(view.wallets, (w) => w.id) as w (w.id)}<option value={w.id}>{w.name}</option>{/each}
          </select>
        </td>
        <td class="muted">
          {#if !r.wallet && r.kind === 'cex'}нет кошелька DEX — {r.row} с DEX не торгует{/if}
          {#if !r.wallet && r.kind === 'dex_dex'}циклы не исполняются{/if}
          {#if !r.wallet && r.kind === 'probe'}пробы налогов и сигналы без кошелька не считаются{/if}
        </td>
      </tr>
    {/each}
  </tbody>
</table>
<ErrorText text={error} />

<h3>История изменений</h3>
<PageSize key="wallets.history" bind:value={limit} label="Записей" />
{#if !limit}
  <p class="muted">Задайте, сколько записей показать.</p>
{:else}
  <table>
    <thead><tr><th>Когда</th><th>Кто</th><th>Что</th><th>Было</th><th>Стало</th></tr></thead>
    <tbody>
      {#each history as h, i (i)}
        <tr>
          <td>{showTime(h.at)}</td>
          <td>{h.by}</td>
          <td>{h.what} {names[h.target] ?? h.target}</td>
          <td><code>{h.old === null ? '—' : JSON.stringify(h.old)}</code></td>
          <td><code>{h.new === null ? '—' : JSON.stringify(h.new)}</code></td>
        </tr>
      {/each}
    </tbody>
  </table>
{/if}
