<!--
  Файл: ui/src/components/ExecutorDeploy.svelte
  Что делает: блок «Свой исполнитель (ArbExecutor)» на экране «Площадки» у DEX (решение 122, часть X): кнопка
    «Развернуть исполнитель» → GET /api/venues/{id}/executor/quote (газ, цена газа, стоимость в BNB и USDT, кошелёк,
    nonce, будущий адрес, SHA-256 байткода, версия solc) → окно подтверждения → «Подтвердить и отправить» →
    POST /api/venues/{id}/executor/deploy {sha256} (транзакция создания с кошелька бота через очередь кошелька) →
    адрес и транзакция. Действие с деньгами — только по кнопке человека и после подтверждения в окне. Адрес в
    exec.executor_address Тигран вписывает сам (настройки площадки ниже на этом же экране). Рядом — «Проверить
    подключение» исполнителя (POST /api/check/executor/{venue}, ревью 122): код по адресу совпал с репозиторием и
    владелец — кошелёк бота; иначе — точная причина (без этой проверки ни approve, ни сделки через исполнитель нет).
  Кто вызывает: pages/Venues.svelte (у площадок DEX).
  От чего зависит: lib/api.js, components/Modal.svelte, components/ErrorText.svelte, components/CheckConnection.svelte.
  Настройки: нет своих (адрес — exec.executor_address [площадка DEX]).
-->
<script>
  import { get, post, seg } from '../lib/api.js';
  import Modal from './Modal.svelte';
  import ErrorText from './ErrorText.svelte';
  import CheckConnection from './CheckConnection.svelte';

  /** venue — id площадки DEX. */
  let { venue } = $props();

  let quote = $state(null);
  let done = $state(null);
  let busy = $state(false);
  let error = $state('');

  /** Оценка развёртывания — ничего не отправляет. */
  async function ask() {
    error = '';
    done = null;
    busy = true;
    try {
      quote = await get(`/api/venues/${seg(venue)}/executor/quote`);
    } catch (e) {
      error = e.message;
    }
    busy = false;
  }

  /** Отправка транзакции создания: в теле — хеш байткода, который человек видел в окне. */
  async function deploy() {
    error = '';
    busy = true;
    try {
      done = await post(`/api/venues/${seg(venue)}/executor/deploy`, { sha256: quote.bytecode_sha256 });
      quote = null;
    } catch (e) {
      error = e.message;
    }
    busy = false;
  }
</script>

<h3>Свой исполнитель (ArbExecutor)</h3>
<p class="muted">
  Контракт из репозитория (contracts/ArbExecutor.sol): маршрут через пулы разных DEX — одной транзакцией. Владелец —
  кошелёк бота, вызывать может только он, денег контракт не хранит. Проверить его до развёртывания — «Сухая сделка DEX»
  (Ручное управление) в режиме «через исполнитель». После развёртывания впишите адрес в <code>exec.executor_address</code>.
</p>
<button disabled={busy} onclick={ask}>{busy && !quote ? 'Считаю…' : 'Развернуть исполнитель'}</button>
<CheckConnection url={`/api/check/executor/${encodeURIComponent(venue)}`} />
<ErrorText text={error} />
{#if done}
  <p class={done.code_ok ? 'ok' : 'err'} role="status">{done.text}</p>
  <p>Адрес: <code>{done.address}</code>, транзакция <code>{done.chain.hash}</code></p>
{/if}

{#if quote}
  <Modal title="Развернуть исполнитель — подтверждение" onclose={() => (quote = null)}>
    <p>С кошелька бота уйдёт транзакция создания контракта. Это трата BNB на газ, отменить её нельзя.</p>
    <table>
      <tbody>
        <tr><th>Кошелёк (владелец)</th><td><code>{quote.wallet}</code></td></tr>
        <tr><th>Адрес контракта (при nonce {quote.nonce})</th><td><code>{quote.address}</code></td></tr>
        <tr><th>Газ (лимит)</th><td>{quote.gas}</td></tr>
        <tr><th>Цена газа</th><td>{quote.gas_price_wei} wei</td></tr>
        <tr><th>Стоимость</th><td>{quote.cost_bnb} BNB{quote.cost_usdt ? ` ≈ ${quote.cost_usdt} USDT` : ''}</td></tr>
        <tr><th>Байткод</th><td>{quote.bytecode_len} байт, SHA-256 <code>{quote.bytecode_sha256}</code>, solc {quote.solc}</td></tr>
      </tbody>
    </table>
    {#if quote.note}<p class="muted">{quote.note}</p>{/if}
    <p class="muted">Если до отправки уйдёт другая транзакция кошелька, адрес будет другим — точный адрес покажется после блока.</p>
    <button disabled={busy} onclick={deploy}>{busy ? 'Отправляю…' : 'Подтвердить и отправить'}</button>
    <button disabled={busy} onclick={() => (quote = null)}>Отмена</button>
  </Modal>
{/if}
