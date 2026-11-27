<!-- Размещение принятого модуля одним CREATE в выбранной сети по действию оператора. -->
<script>
  import {store, setValue} from '../lib/settings.svelte.js';
  import {loadPlan, unsigned, estimate, send, deployed} from '../lib/walletDeployment.js';
  import ErrorText from './ErrorText.svelte';
  let {id, kind, venue} = $props();
  const stabullPlan = {file:'stabull-base',key:'stabull-create:8453',name:'StabullAdapter на Base',load:()=>import('../assets/stabull-base.json')};
  const deployments = {
    uniswap_v1:{file:'uniswap-v1-ethereum',key:'uniswap-v1-create:1',name:'UniswapV1Adapter в Ethereum',load:()=>import('../assets/uniswap-v1-ethereum.json')},
    bancor_v3:{file:'bancor-v3-ethereum',key:'bancor-v3-create:1',name:'BancorV3Adapter в Ethereum',load:()=>import('../assets/bancor-v3-ethereum.json')},
    balancer_v1:{file:'balancer-v1-ethereum',key:'balancer-v1-create:1',name:'BalancerV1Adapter в Ethereum',load:()=>import('../assets/balancer-v1-ethereum.json')},
    stabull:stabullPlan, stabull_module:stabullPlan,
    rubicon_classic:{file:'rubicon-classic-base',key:'rubicon-classic-create:8453',name:'RubiconClassicWallet на Base'},
  };
  const deployment = $derived(deployments[kind]);
  const recoverable = $derived(!!deployment.load);
  const file = $derived(deployment.file);
  let manifest = $state(null), estimateSeen = $state(null), pending = $state(null), txHash = $state('');
  let result = $state(''), error = $state(''), busy = $state(false), readyAddress = $state('');
  const key = $derived(deployment.key);
  const value = (name) => store.values[name+'|Dex|'+id];
  const atVenue = (name) => store.values[name+'|Venue|'+venue];
  const current = () => ({chainId:atVenue('dex.evm_chain_id'), wrapped:atVenue('dex.native_wrapped'),
    factoryHash:value('dex.factory_runtime_hash'), template:value('dex.v1_template'),
    templateHash:value('dex.v1_template_runtime_hash'), cloneHash:value('dex.pair_runtime_hash'), kind:value('dex.kind'), market:value('dex.factory'),
    quoter:value('dex.quoter'), vault:value('dex.vault'), native:value('dex.bancor_native_token'), roles:value('dex.bancor_roles'),
    hashes:value('dex.module_runtime_hashes'), implementations:value('dex.proxy_implementations')});
  function refreshPending() {
    pending = JSON.parse(localStorage.getItem(key) ?? 'null');
    if (pending?.replacementHash || pending?.hash) txHash = pending.replacementHash ?? pending.hash;
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
    if (recoverable) {
      const artifact = await deployment.load();
      manifest = await loadPlan(current(),artifact.default);
    } else {
      const response = await fetch(import.meta.env.BASE_URL+file+'.json');
      if (!response.ok) throw new Error('Архив размещения отсутствует в этой сборке.');
      manifest = await loadPlan(current(), await response.json());
    }
  }
  async function download() {
    await loadPlan(current(),manifest);
    const url = URL.createObjectURL(new Blob([JSON.stringify(unsigned(manifest), null, 2)], {type:'application/json'}));
    const a=document.createElement('a'); a.href=url; a.download=file+'-create.json'; a.click();
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
  function sameIntent(a,b) {
    if (!a || !b) return a===b;
    return a.from===b.from && a.chain===b.chain && a.nonce===b.nonce && a.creation===b.creation;
  }
  async function reconcile(expected,act) {
    if (!recoverable) return act();
    const compare=async()=>{
      refreshPending();
      if (!sameIntent(pending,expected)) throw Error('Исходное поручение изменилось в другой вкладке. Повторите проверку.');
      return act();
    };
    if (navigator.locks) return navigator.locks.request(key,compare);
    return compare(); // Browsers without Web Locks can verify/export, but cannot submit a new request.
  }
  async function verify() {
    const supplied=txHash.trim();
    refreshPending();
    const original=pending;
    readyAddress='';
    await loadPlan(current(), manifest);
    const fact = await deployed(provider(), manifest, supplied, original);
    await reconcile(original,async()=>{
      if (fact.consumed) {
        localStorage.setItem(key+':lastResolution',JSON.stringify(fact));
        save(null); txHash=supplied; estimateSeen=null;
        result=fact.earlierCreation && fact.succeeded
          ? 'Финализированный CREATE относится к прежней версии модуля. Для действующей версии требуется отдельное размещение.'
          : 'Исходный nonce занят финализированной транзакцией. Размещение текущего модуля по этому поручению завершено без установки.';
      } else if (fact.reverted) {
        if (pending) save(null);
        txHash=supplied; estimateSeen=null;
        result='Финализированная транзакция завершилась откатом.';
      } else {
        readyAddress=fact.address;
        if (fact.replacementHash && pending) save({...pending,replacementHash:fact.replacementHash});
        result=fact.replacementHash ? 'Замена CREATE с исходным nonce финализирована. Код модуля подтверждён.' : 'Каноническая транзакция и код модуля подтверждены.';
      }
    });
    return original;
  }
  async function useAddress() {
    const original=await verify();
    if (!readyAddress) throw new Error('Подтверждённого адреса модуля нет.');
    await reconcile(original,async()=>{
      await loadPlan(current(),manifest);
      if (value('dex.router')) throw new Error('Адрес модуля уже настроен.');
      await setValue('dex.router', {kind:'Dex', id}, readyAddress);
      save(null);
      result=kind === 'stabull' ? 'Адрес сохранён. Для защищённых обменов выберите вид stabull_module и сохраните настройки DEX.' : 'Подтверждённый адрес сохранён в dex.router.';
    });
  }

</script>

<details>
  <summary>Разместить {deployment.name}</summary>
  <button class="quiet" disabled={busy} onclick={() => run(prepare)}>Открыть параметры размещения</button>
  {#if manifest}
    {#if kind === 'balancer_v1'}
      <p>Фабрика BPool <code>{manifest.factory}</code>, WETH <code>{manifest.wrapped}</code>. При создании кошелёк отправляет 0 ETH.</p>
    {:else if deployment === stabullPlan}
      <p>Конструктор принимает адрес фабрики <code>{manifest.market}</code>. Сумма перевода при создании — 0 ETH.</p>
    {:else if kind === 'bancor_v3'}
      <p>Конструктор Bancor V3 в Ethereum. Перевод ETH при создании — 0.</p>
      {#each ['BancorNetwork','BancorNetworkInfo','MasterVault','WETH','Нативный ETH'] as label, i}
        <div>{label} <code>{manifest.arguments[i]}</code></div>
      {/each}
    {:else if kind === 'uniswap_v1'}
      <p>Фабрика <code>{manifest.roles.factory.address}</code>, WETH <code>{manifest.roles.wrapped.address}</code>. Конструктор принимает оба адреса. Перевод ETH контракту — 0.</p>
    {:else}
      <p>Рынок <code>{manifest.market}</code>. Конструктор принимает этот адрес. Перевод ETH контракту — 0.</p>
    {/if}
    <p>Хеш проверенного runtime <code>{manifest.runtimeKeccak}</code>.</p>
    <button class="quiet" disabled={busy} onclick={() => run(download)}>Скачать неподписанный CREATE</button>
    {#if !pending && !value('dex.router')}
      <button class="quiet" disabled={busy} onclick={() => run(price)}>Оценить газ в кошельке</button>
      {#if estimateSeen}
        <p>Плательщик <code>{estimateSeen.from}</code>. Оценка {manifest.chainId === 1 ? "Ethereum" : "L2"} — {estimateSeen.gas} ед. газа, {estimateSeen.fee} wei. Кошелёк покажет условия отправки и сетевую комиссию.</p>
        <button disabled={busy} onclick={() => run(submit)}>Подтвердить создание контракта в кошельке</button>
      {/if}
    {/if}
    {#if pending}<p>Отправка начата с <code>{pending.from}</code>, nonce {pending.nonce}. {recoverable ? "Проверьте исходную транзакцию или замену с тем же nonce." : "Проверьте исходную транзакцию."}</p>{/if}
    <label>{pending && recoverable ? "Хеш CREATE или его замены" : "Хеш CREATE"} <input bind:value={txHash} placeholder="0x…" /></label>
    <button class="quiet" disabled={busy || !txHash.trim()} onclick={() => run(verify)}>Проверить размещение</button>
    {#if readyAddress}
      <p>Адрес модуля <code>{readyAddress}</code></p>
      <button disabled={busy || !!value('dex.router')} onclick={() => run(useAddress)}>Сохранить адрес модуля</button>
    {/if}
  {/if}
  {#if result}<p>{result}</p>{/if}
  <ErrorText text={error} />
</details>
