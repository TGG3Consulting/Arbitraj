<!--
  Файл: ui/src/components/CoinDirectory.svelte
  Что делает: в окне монеты — пулы монеты из справочника индексатора (решение 58): DEX индексатора, пара, адрес или
    PoolId коротко, ликвидность по индексатору и что сделал бот (взят / не проверен / мельче порога / «DEX X не
    подключена» / отклонён с причиной). Цены и состояние пулов — только со своего узла (блок «Пулы» ниже).
  Кто вызывает: components/CoinModal.svelte.
  От чего зависит: lib/dex.svelte.js (directoryPools), lib/directory.js, lib/fmt.js, components/Attribution.svelte.
  Настройки: нет (index.* — экран «Площадки»).
-->
<script>
  import { directoryPools } from '../lib/dex.svelte.js';
  import { dirStatus, usdShort } from '../lib/directory.js';
  import { shortAddr } from '../lib/fmt.js';
  import Attribution from './Attribution.svelte';

  /** addr — адрес контракта монеты (или null). */
  let { addr } = $props();
  const list = $derived(directoryPools(addr));
  const attr = $derived(list.find((x) => x.attribution)?.attribution);
</script>

<h4>Справочник пулов (индексатор)</h4>
{#if list.length}
  <table class="dir">
    <tbody>
      {#each list as { venue, pool } (venue + pool.pool)}
        {@const st = dirStatus(pool.status)}
        <tr>
          <td><code>{pool.dex_id}</code></td>
          <td>{pool.base_symbol}/{pool.quote_symbol}</td>
          <td><code title={pool.pool}>{shortAddr(pool.pool)}</code></td>
          <td>{usdShort(pool.reserve_usd)}</td>
          <td class={st.cls}>{st.text}</td>
        </tr>
      {/each}
    </tbody>
  </table>
  <Attribution text={attr?.text} url={attr?.url} />
{:else}
  <p class="muted">Нет пулов с USDT или промежуточной монетой в справочнике (или справочник ещё не получен — «Площадки» → DEX).</p>
{/if}

<style>
  .dir td { padding: 2px 8px 2px 0; vertical-align: top; }
  .warn { color: var(--unset); }
</style>
