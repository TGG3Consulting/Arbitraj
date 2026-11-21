<!--
  Файл: ui/src/components/WithdrawBlocked.svelte
  Что делает: решение 253, п. (г) — на экране «Монеты»: монеты, у которых входы с биржи закрыты после окончательного
    отказа вывода (coin.entry_blocked_venues — список бирж, coin.entry_blocked_note — причина; пишет ядро,
    core/withdraw_block.rs). У каждой биржи — «Открыть» (биржа убирается из списка; последняя — обе настройки
    очищаются). Работает через обычный PUT /api/settings (область «монета»), изменения видны сразу во всех вкладках и
    пишутся в историю настроек.
  Кто вызывает: pages/Coins.svelte.
  От чего зависит: lib/settings.svelte.js, lib/coinNames.svelte.js, components/ErrorText.svelte.
  Настройки: coin.entry_blocked_venues, coin.entry_blocked_note (src/settings/catalog/trade.rs).
-->
<script>
  import { store, setValue, defOf } from '../lib/settings.svelte.js';
  import { names, coinName, ensure } from '../lib/coinNames.svelte.js';
  import ErrorText from './ErrorText.svelte';

  ensure();

  const VENUES = 'coin.entry_blocked_venues';
  const NOTE = 'coin.entry_blocked_note';

  let error = $state('');
  let busy = $state(false);

  /** Монеты с закрытыми входами: [{id, venues, note}] по алфавиту. */
  const list = $derived.by(() => {
    const prefix = `${VENUES}|Coin|`;
    return Object.entries(store.values)
      .filter(([k, v]) => k.startsWith(prefix) && Array.isArray(v) && v.length)
      .map(([k, v]) => {
        const id = k.slice(prefix.length);
        return { id, venues: v, note: store.values[`${NOTE}|Coin|${id}`] };
      })
      .sort((a, b) => a.id.localeCompare(b.id));
  });

  /** Открывает входы монеты с биржи: биржа убирается из списка; последняя — обе настройки очищаются. */
  async function open(c, venue) {
    busy = true;
    error = '';
    try {
      const scope = { kind: 'Coin', id: c.id };
      const rest = c.venues.filter((v) => v !== venue);
      if (rest.length) {
        await setValue(VENUES, scope, rest);
      } else {
        await setValue(VENUES, scope, null);
        await setValue(NOTE, scope, null);
      }
    } catch (e) {
      error = e.message;
    }
    busy = false;
  }
</script>

<section class="card">
  <h2>Входы закрыты после отказа вывода</h2>
  {#if !defOf(VENUES) || !defOf(NOTE)}
    <p class="err">Настроек {VENUES} / {NOTE} нет в схеме бэкенда.</p>
  {:else}
    <p class="muted">{defOf(VENUES).description}</p>
    {#each list as c (c.id)}
      <div class="row">
        <strong>{names.map[c.id] ? coinName({ ticker: names.map[c.id].ticker, bsc_contract: c.id }) : c.id}</strong>
        <span class={c.note ? '' : 'muted'}>{c.note ?? 'причина не указана'}</span>
        {#each c.venues as v (v)}
          <button class="quiet" disabled={busy} onclick={() => open(c, v)}>Открыть {v}</button>
        {/each}
      </div>
    {:else}
      <p class="muted">Закрытых входов нет.</p>
    {/each}
    <ErrorText text={error} />
  {/if}
</section>
