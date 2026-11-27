<!-- Размещение принятого кошелька Rubicon Classic одним CREATE в Base по явному действию оператора. -->
<script>
  import {store, setValue} from '../lib/settings.svelte.js';
  import {loadPlan, unsigned, estimate, send, deployed} from '../lib/rubiconDeploy.js';
  import ErrorText from './ErrorText.svelte';
  let {id} = $props();
  let manifest = $state(null), estimateSeen = $state(null), pending = $state(null), txHash = $state('');
  let result = $state(''), error = $state(''), busy = $state(false), readyAddress = $state('');
  const key = 'rubicon-classic-create:8453';
  const value = (name) => store.values[name+'|Dex|'+id];
  const current = () => ({kind:value('dex.kind'), market:value('dex.factory'),
    hashes:value('dex.module_runtime_hashes'), implementations:value('dex.proxy_implementations')});
  function refreshPending() {
    pending = JSON.parse(localStorage.getItem(key) ?? 'null');
    if (pending?.hash) txHash = pending.hash;
  }
  function save(p) {
    if (p) localStorage.setItem(key, JSON.stringify(p));
    else localStorage.removeItem(key);
    refreshPending();
  }
  function provider() {
    if (!window.ethereum?.request) throw new Error('Для проверки или отправки подключите Ethereum-кошелёк в браузере.');
    return window.ethereum;
  }
  async function run(fn) {
    if (busy) return;
    busy = true; error = '';
    try { await fn(); } catch (e) { error = e.message; } finally { busy = false; }
  }
  async function prepare() {
    refreshPending();
    const response = await fetch(import.meta.env.BASE_URL+'rubicon-classic-base.json');
    if (!response.ok) throw new Error('Архив размещения отсутствует в этой сборке.');
    manifest = await loadPlan(current(), await response.json());
  }
  async function download() {
    await loadPlan(current(),manifest);
    const url = URL.createObjectURL(new Blob([JSON.stringify(unsigned(manifest), null, 2)], {type:'application/json'}));
    const a=document.createElement('a'); a.href=url; a.download='rubicon-classic-base-create.json'; a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
  async function price() {
    await loadPlan(current(), manifest);
    estimateSeen = await estimate(provider(), manifest);
  }
  async function submit() {
    if (!navigator.locks) throw new Error('Для отправки из браузера требуется защищённое соединение с поддержкой Web Locks. Доступен экспорт.');
    await navigator.locks.request(key, {ifAvailable:true}, async (lock) => {
      refreshPending();
      if (!lock || pending || value('dex.router')) throw new Error('Есть ранее начатое размещение или настроенный модуль.');
      await loadPlan(current(), manifest);
      txHash = await send(provider(), manifest, estimateSeen, save);
      result = 'Транзакция передана кошельком. После финализации проверьте её по хешу.';
    });
  }
  async function verify() {
    const supplied=txHash.trim();
    refreshPending();
    await loadPlan(current(), manifest);
    const fact = await deployed(provider(), manifest, supplied, pending);
    if (fact.reverted) { if (pending) save(null); readyAddress=''; result='Финализированная транзакция завершилась откатом.'; }
    else { readyAddress=fact.address; result='Каноническая транзакция и код модуля подтверждены.'; }
  }
  async function useAddress() {
    await verify();
    if (!readyAddress || value('dex.router')) throw new Error('Адрес уже настроен или подтверждение отсутствует.');
    await setValue('dex.router', {kind:'Dex', id}, readyAddress);
    save(null);
    result='Подтверждённый адрес сохранён в dex.router.';
  }
</script>

<details>
  <summary>Разместить RubiconClassicWallet на Base</summary>
  <button class="quiet" disabled={busy} onclick={() => run(prepare)}>Открыть параметры размещения</button>
  {#if manifest}
    <p>Рынок <code>{manifest.market}</code>. Конструктор принимает этот адрес. Перевод ETH контракту — 0.</p>
    <p>Хеш проверенного runtime <code>{manifest.runtimeKeccak}</code>.</p>
    <button class="quiet" disabled={busy} onclick={() => run(download)}>Скачать неподписанный CREATE</button>
    {#if !pending && !value('dex.router')}
      <button class="quiet" disabled={busy} onclick={() => run(price)}>Оценить газ в кошельке</button>
      {#if estimateSeen}
        <p>Плательщик <code>{estimateSeen.from}</code>. Оценка L2 — {estimateSeen.gas} ед. газа, {estimateSeen.fee} wei. Кошелёк покажет условия отправки и сетевую комиссию.</p>
        <button disabled={busy} onclick={() => run(submit)}>Подтвердить создание контракта в кошельке</button>
      {/if}
    {/if}
    {#if pending}<p>Отправка начата с <code>{pending.from}</code>, nonce {pending.nonce}. Проверьте исходную транзакцию.</p>{/if}
    <label>Хеш CREATE <input bind:value={txHash} placeholder="0x…" /></label>
    <button class="quiet" disabled={busy || !txHash.trim()} onclick={() => run(verify)}>Проверить размещение</button>
    {#if readyAddress}
      <p>Адрес модуля <code>{readyAddress}</code></p>
      <button disabled={busy || !!value('dex.router')} onclick={() => run(useAddress)}>Сохранить адрес модуля</button>
    {/if}
  {/if}
  {#if result}<p>{result}</p>{/if}
  <ErrorText text={error} />
</details>
