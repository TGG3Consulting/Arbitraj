<!--
  Файл: ui/src/components/ExecutorDeploy.svelte
  Что делает: исполнитель ArbExecutor кошелька DEX на экране «Маппинг кошельков» (решения 122 X, 132: у каждого кошелька
    свой исполнитель — запросы с ?wallet=<id>; адрес после развёртывания записывается у кошелька сам): кнопка
    «Развернуть исполнитель» → GET /api/venues/{id}/executor/quote (газ, цена газа, стоимость в BNB и USDT, кошелёк,
    nonce, будущий адрес, SHA-256 байткода, версия solc, адрес обёртки BNB — аргумент конструктора, ревью 272 А1) →
    окно подтверждения → «Подтвердить и отправить» →
    POST /api/venues/{id}/executor/deploy {sha256, wnative} (задача t114: и адрес обёртки из окна — бот сверяет его с
    текущей dex.native_wrapped; транзакция создания с кошелька бота через очередь кошелька) →
    адрес и транзакция. Действие с деньгами — только по кнопке человека и после подтверждения в окне. Код в сети
    совпал — адрес записан у кошелька (решение 132). Рядом — «Проверить
    подключение» исполнителя (POST /api/check/executor/{venue}, ревью 122): код по адресу совпал с репозиторием и
    владелец — кошелёк бота; иначе — точная причина (без этой проверки ни approve, ни сделки через исполнитель нет).
    Ревью t181 (Н8): в окне — предупреждение quote.warning (сборка исполнителя ещё не принята на живых пулах — нет записи
    симуляции в tests/data; бэкенд: exec/executor/deploy_gate.rs refusal).
    Решение 399: пока сборка не принята на живых пулах, развёртывание запрещено (сервер отказывает, обходной настройки
    нет): refused — причина из GET /api/wallets (executor_deploy_refused) — кнопка «Развернуть исполнитель» недоступна,
    причина видна рядом; quote.warning (та же причина, пришла с оценкой) — «Подтвердить и отправить» недоступна.
    Решение 400: classes — классы шага исполнителя v3 (вид × крючок) из GET /api/wallets (executor_v3_classes) и
    quote.classes: таблица «Классы шага v3» — принят / не принят, пулов доказано из скольких, почему. Развёртывание
    разрешено, если принят хотя бы один класс; шаги непринятых классов исполнитель не берёт.
    Решение 401: колонка «Пулы кошелька» (поле live) — проба пулов класса через исполнитель v3 ЭТОГО кошелька: у
    непринятого класса пул исполнителем идёт, только если прошёл пробу обе стороны (прошли / отвергнуты / не проверено);
    у принятого — сколько пулов отвергнуто пробой или откатом сделки. Подробно по пулу — карточка пула на «Монетах».
  Кто вызывает: pages/Wallets.svelte (у каждого кошелька).
  От чего зависит: lib/api.js, components/Modal.svelte, components/ErrorText.svelte, components/CheckConnection.svelte.
  Настройки: нет (адрес исполнителя — у кошелька).
-->
<script>
  import { get, post, seg } from '../lib/api.js';
  import Modal from './Modal.svelte';
  import ErrorText from './ErrorText.svelte';
  import CheckConnection from './CheckConnection.svelte';

  /** venue — id площадки DEX; wallet — id кошелька (решение 132); refused — причина запрета развёртывания (решение 399,
   *  null — разрешено); classes — классы шага v3 и их приёмка (решение 400); ondone — после записи адреса. */
  let { venue, wallet, refused = null, classes = [], ondone } = $props();
  const q = $derived(`?wallet=${seg(wallet)}`);

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
      quote = await get(`/api/venues/${seg(venue)}/executor/quote${q}`);
    } catch (e) {
      error = e.message;
    }
    busy = false;
  }

  /** Отправка транзакции создания: в теле — хеш байткода и адрес обёртки (задача t114), которые человек видел в окне. */
  async function deploy() {
    error = '';
    busy = true;
    try {
      done = await post(`/api/venues/${seg(venue)}/executor/deploy${q}`, {
        sha256: quote.bytecode_sha256,
        wnative: quote.wnative,
      });
      quote = null;
      ondone?.();
    } catch (e) {
      error = e.message;
    }
    busy = false;
  }
</script>

<button disabled={busy || !!refused} title={refused ?? ''} onclick={ask}>{busy && !quote ? 'Считаю…' : 'Развернуть исполнитель'}</button>
<CheckConnection url={`/api/check/executor/${encodeURIComponent(venue)}${q}`} />
{#if refused}<p class="warn-text">«Развернуть исполнитель» недоступна: {refused}</p>{/if}
{#snippet classTable(list)}
  <table>
    <caption>Классы шага v3 (запись симуляции на живых пулах, решение 400; проба пулов через исполнитель кошелька, решение 401)</caption>
    <thead><tr><th>Класс</th><th>Пулов доказано</th><th>Принят</th><th>Почему</th><th>Пулы кошелька</th></tr></thead>
    <tbody>
      {#each list as c (c.class)}
        <tr><td>{c.class}</td><td>{c.proven} из {c.pools}</td><td class={c.accepted ? 'ok' : 'warn-text'}>{c.accepted ? 'да' : 'нет'}</td><td>{c.why}</td><td>{c.live || '—'}</td></tr>
      {/each}
    </tbody>
  </table>
{/snippet}
{#if classes.length}{@render classTable(classes)}{/if}
<ErrorText text={error} />
{#if done}
  <p class={done.code_ok ? 'ok' : 'err'} role="status">{done.text}</p>
  <p>Адрес: <code>{done.address}</code>, транзакция <code>{done.chain.hash}</code></p>
{/if}

{#if quote}
  <Modal title="Развернуть исполнитель — подтверждение" onclose={() => (quote = null)}>
    <p>С этого кошелька уйдёт транзакция создания контракта (владелец — он). Это трата BNB на газ, отменить её нельзя.</p>
    <table>
      <tbody>
        <tr><th>Кошелёк (владелец)</th><td><code>{quote.wallet}</code></td></tr>
        <tr><th>Адрес контракта (при nonce {quote.nonce})</th><td><code>{quote.address}</code></td></tr>
        <tr><th>Газ (лимит)</th><td>{quote.gas}</td></tr>
        <tr><th>Цена газа</th><td>{quote.gas_price_wei} wei</td></tr>
        <tr><th>Стоимость</th><td>{quote.cost_bnb} BNB{quote.cost_usdt ? ` ≈ ${quote.cost_usdt} USDT` : ''}</td></tr>
        <tr><th>Байткод</th><td>{quote.bytecode_len} байт, SHA-256 <code>{quote.bytecode_sha256}</code>, solc {quote.solc}</td></tr>
        <tr><th>Обёртка BNB (навсегда)</th><td><code>{quote.wnative}</code> — dex.native_wrapped, аргумент конструктора; сменить потом нельзя, только переразвернуть</td></tr>
      </tbody>
    </table>
    {#if quote.warning}<p class="warn-text">{quote.warning}</p>{/if}
    {#if quote.classes?.length}{@render classTable(quote.classes)}{/if}
    {#if quote.note}<p class="muted">{quote.note}</p>{/if}
    <p class="muted">Если до отправки уйдёт другая транзакция кошелька, адрес будет другим — точный адрес покажется после блока.</p>
    <button disabled={busy || !!quote.warning} title={quote.warning ?? ''} onclick={deploy}>{busy ? 'Отправляю…' : 'Подтвердить и отправить'}</button>
    <button disabled={busy} onclick={() => (quote = null)}>Отмена</button>
  </Modal>
{/if}
