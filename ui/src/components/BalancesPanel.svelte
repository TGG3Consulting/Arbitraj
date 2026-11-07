<!--
  Файл: ui/src/components/BalancesPanel.svelte
  Что делает: панель «Балансы» (решение 144, Тигран 04.11): кнопка справа в верхнем меню открывает закреплённую панель
    поверх экрана — она не закрывается при переходе по экранам и при клике мимо, только повторным нажатием «Балансы»
    (открыта ли — запоминается в браузере). В панели — каждая биржа с разбивкой по счетам (спот, фьючерсы, фонд, earn…
    — как отдаёт адаптер) и кошельки DEX (BNB и USDT). Пока панель открыта, каждая биржа и каждый кошелёк обновляются
    раз в ui.balances_refresh_ms (следующий запрос — только после ответа предыдущего); кнопка ⟳ — обновить сразу, вне
    очереди. Сбой — причина и прежние значения.
    Решение 174: у каждой монеты биржи (кроме котируемой) — кнопка «Продать»: подтверждение, затем команда ядру
    SellBalance (всё свободное по рынку в USDT; не со спота — сначала перевод на торговый счёт; выручка — на споте).
    Недоступна — disabled, причина по наведению (поле sell ответа). Итог — над балансами биржи; баланс обновляется сразу.
    Решение 176: «Перевести USDT» — форма components/UsdtTransfer.svelte (та же команда ядра, что в Telegram).
  Кто вызывает: App.svelte.
  От чего зависит: lib/api.js (GET /api/balances, /api/balances/{id}, /api/wallets, /api/wallets/{id}/balances,
    POST /api/manual/command),
    lib/prefs.js, lib/fmt.js.
  Настройки: ui.balances_refresh_ms (не задана — только кнопки ⟳, причина видна в панели).
-->
<script>
  import { onDestroy, untrack } from 'svelte';
  import { get, post, seg } from '../lib/api.js';
  import { load, save } from '../lib/prefs.js';
  import UsdtTransfer from './UsdtTransfer.svelte';

  let open = $state(load('balances.open') === '1');
  let refreshMs = $state(null);
  let refreshError = $state('');
  let listError = $state('');
  let transfer = $state(false);
  /** Строки панели: {key, kind: 'cex'|'dex', id, title, data, error, at, busy}. */
  let rows = $state([]);
  /** Будильники ожидания по key: ⟳ будит сразу. */
  const wake = new Map();
  let gen = 0;

  /** Открыть/скрыть панель. Ошибок нет. */
  function toggle() {
    open = !open;
    save('balances.open', open ? '1' : '0');
  }

  /** Пауза автообновления строки; ⟳ прерывает её. Принимает: key, мс (null — ждать только ⟳). */
  function pause(key, ms) {
    return new Promise((res) => {
      const t = ms ? setTimeout(res, ms) : null;
      wake.set(key, () => {
        if (t) clearTimeout(t);
        res();
      });
    });
  }

  /** Один запрос строки. Принимает: строку. Ошибки: в row.error, прежние данные остаются. */
  async function fetchRow(r) {
    r.busy = true;
    try {
      if (r.kind === 'cex') {
        const d = await get(`/api/balances/${seg(r.id)}`);
        if (d.error && !d.accounts.length) r.error = d.error;
        else {
          r.data = d.accounts;
          r.error = d.error ?? '';
        }
      } else {
        const d = await get(`/api/wallets/${seg(r.id)}/balances`);
        r.data = d;
        r.error = d.error ?? '';
      }
      r.at = new Date();
    } catch (e) {
      r.error = e.message;
    } finally {
      r.busy = false;
    }
  }

  /** Круг одной строки, пока панель открыта и поколение то же. Ошибок нет. */
  async function loop(r, my) {
    while (open && my === gen) {
      await fetchRow(r);
      if (!open || my !== gen) break;
      if (r.again) {
        r.again = false;
        continue;
      }
      await pause(r.key, refreshMs);
    }
  }

  /** ⟳: обновить строку сразу (если идёт запрос — следующий начнётся сразу после него). */
  function refresh(r) {
    if (r.busy) r.again = true;
    else wake.get(r.key)?.();
  }

  /** Запуск панели: список бирж и кошельков, затем круги. Ошибки: listError. */
  async function start() {
    const my = ++gen;
    listError = '';
    try {
      const [b, w] = await Promise.all([get('/api/balances'), get('/api/wallets').catch(() => ({ wallets: [] }))]);
      refreshMs = b.refresh_ms;
      refreshError = b.refresh_error ?? '';
      const old = new Map(rows.map((r) => [r.key, r]));
      const keep = (key, init) => old.get(key) ?? init;
      rows = [
        ...uniq(b.venues).map((id) => keep(`cex:${id}`, { key: `cex:${id}`, kind: 'cex', id, title: id, data: null, error: '', at: null, busy: false, again: false, sold: null, selling: '' })),
        ...(w.wallets ?? []).map((x) =>
          keep(`dex:${x.id}`, { key: `dex:${x.id}`, kind: 'dex', id: x.id, title: `Кошелёк DEX «${x.name}»`, data: null, error: '', at: null, busy: false, again: false }),
        ),
      ];
    } catch (e) {
      listError = e.message;
      return;
    }
    for (const r of rows) loop(r, my);
  }

  $effect(() => {
    if (open) untrack(() => start());
    else {
      gen++;
      for (const f of wake.values()) f();
    }
  });
  onDestroy(() => {
    gen++;
    for (const f of wake.values()) f();
  });

  /**
   * «Продать» (решение 174): подтверждение, команда ядру, итог в строке биржи, затем обновление баланса.
   * Принимает: строку биржи, счёт, строку баланса. Ошибки: текст отказа — в r.sold.
   */
  async function sell(r, a, b) {
    if (!confirm(`Продать ${num(b.free)} ${b.asset} на ${r.id} по рынку? Выручка USDT — на спот.`)) return;
    r.selling = `${a.account}|${b.asset}`;
    try {
      r.sold = await post('/api/manual/command', {
        action: { SellBalance: { venue: r.id, account: a.account, asset: b.asset } },
      });
    } catch (e) {
      r.sold = { ok: false, details: e.message };
    } finally {
      r.selling = '';
    }
    refresh(r);
  }

  const time = (d) => (d ? d.toLocaleTimeString() : '—');
  /** Число без лишних нулей. */
  const num = (s) => {
    const n = Number(s);
    return Number.isFinite(n) ? n.toLocaleString('ru-RU', { maximumFractionDigits: 8 }) : s;
  };
</script>

<button class="toggle" class:on={open} onclick={toggle}>Балансы</button>

{#if open}
  <aside class="panel" aria-label="Балансы">
    <div class="head">
      <strong>Балансы</strong>
      <span class="muted">
        {#if refreshMs}обновление раз в {refreshMs} мс{:else}автообновления нет: {refreshError}{/if}
      </span>
    </div>
    <button class="small" onclick={() => (transfer = !transfer)}>{transfer ? 'Скрыть перевод' : 'Перевести USDT'}</button>
    {#if transfer}<UsdtTransfer onDone={() => rows.forEach(refresh)} />{/if}
    {#if listError}<p class="err">{listError}</p>{/if}
    {#each rows as r (r.key)}
      <section>
        <div class="row-head">
          <strong>{r.title}</strong>
          <span class="muted small">{time(r.at)}</span>
          <button class="re" class:spin={r.busy} title="Обновить сейчас" onclick={() => refresh(r)}>⟳</button>
        </div>
        {#if r.error}<p class="err small">{r.error}</p>{/if}
        {#if r.sold}<p class="small" class:err={!r.sold.ok} class:ok={r.sold.ok}>{r.sold.details}</p>{/if}
        {#if r.kind === 'cex' && r.data}
          {#each r.data as a, ai (`${a.account}|${ai}`)}
            <div class="acc">
              <div class="acc-name">{a.account}</div>
              {#if a.error}<div class="err small">{a.error}</div>{/if}
              {#if a.note}<div class="muted small">{a.note}</div>{/if}
              {#if a.balances.length}
                <table>
                  <tbody>
                    {#each a.balances as b, bi (`${b.asset}|${bi}`)}
                      <tr>
                        <td>{b.asset}</td>
                        <td class="n">{num(b.free)}</td>
                        <td class="n muted">{Number(b.locked) ? `заморожено ${num(b.locked)}` : ''}</td>
                        <td class="s">
                          {#if b.sell}
                            <button
                              class="sell"
                              disabled={!b.sell.ok || !!r.selling}
                              title={b.sell.ok ? `Продать всё свободное ${b.asset} по рынку в USDT` : b.sell.why}
                              onclick={() => sell(r, a, b)}
                            >{r.selling === `${a.account}|${b.asset}` ? 'Продаётся…' : 'Продать'}</button>
                          {/if}
                        </td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              {:else if !a.error}
                <div class="muted small">пусто</div>
              {/if}
            </div>
          {/each}
        {:else if r.kind === 'dex' && r.data}
          <table>
            <tbody>
              <tr><td>BNB</td><td class="n">{r.data.bnb == null ? '—' : num(r.data.bnb)}</td></tr>
              <tr><td>{r.data.quote ?? 'USDT'}</td><td class="n">{r.data.usdt == null ? '—' : num(r.data.usdt)}</td></tr>
            </tbody>
          </table>
        {:else if !r.error}
          <p class="muted small">загрузка…</p>
        {/if}
      </section>
    {/each}
  </aside>
{/if}

<style>
  .toggle {
    margin-left: auto;
  }
  .toggle.on {
    background: var(--accent);
    color: #fff;
  }
  .panel {
    position: fixed;
    top: 3rem;
    right: 1rem;
    width: min(26rem, calc(100vw - 2rem));
    max-height: calc(100vh - 4rem);
    overflow-y: auto;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 6px;
    box-shadow: 0 6px 24px rgb(0 0 0 / 0.18);
    padding: 0.5rem 0.75rem;
    z-index: 1000;
    font-size: 0.9rem;
  }
  .head {
    display: flex;
    gap: 0.5rem;
    align-items: baseline;
    justify-content: space-between;
    border-bottom: 1px solid var(--line);
    padding-bottom: 0.35rem;
  }
  section {
    border-bottom: 1px solid var(--line);
    padding: 0.4rem 0;
  }
  .row-head {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }
  .row-head strong {
    flex: 1;
    text-transform: none;
  }
  .re {
    padding: 0 0.45rem;
    font-size: 1rem;
    line-height: 1.4;
  }
  .re.spin {
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .acc {
    margin: 0.25rem 0 0.25rem 0.5rem;
  }
  .acc-name {
    font-weight: 600;
    color: var(--muted);
  }
  table {
    width: 100%;
    border-collapse: collapse;
  }
  td {
    padding: 0.05rem 0.25rem;
  }
  .n {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .small {
    font-size: 0.8rem;
  }
  .s {
    text-align: right;
    width: 1%;
    white-space: nowrap;
  }
  .sell {
    padding: 0 0.4rem;
    font-size: 0.8rem;
  }
  .ok {
    color: var(--ok, inherit);
    margin: 0.15rem 0;
  }
  .muted {
    color: var(--muted);
  }
  .err {
    color: var(--err);
    margin: 0.15rem 0;
  }
</style>
