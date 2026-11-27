<!-- Размещение принятого модуля одним CREATE в выбранной сети по действию оператора. -->
<script>
  import {onDestroy} from 'svelte';
  import {deploymentScope} from '../lib/deploymentScope.js';
  import {store, setValue} from '../lib/settings.svelte.js';
  import {loadPlan, unsigned, estimate, send, deployed} from '../lib/walletDeployment.js';
  import ErrorText from './ErrorText.svelte';
  let {id, kind, venue} = $props();
  const stabullPlan = {file:'stabull-base',key:'stabull-create:8453',name:'StabullAdapter на Base',load:()=>import('../assets/stabull-base.json')};
  const deployments = {
    uniswap_v1:{file:'uniswap-v1-ethereum',key:'uniswap-v1-create:1',name:'UniswapV1Adapter в Ethereum',load:()=>import('../assets/uniswap-v1-ethereum.json')},
    bancor_v3:{file:'bancor-v3-ethereum',key:'bancor-v3-create:1',name:'BancorV3Adapter в Ethereum',load:()=>import('../assets/bancor-v3-ethereum.json')},
    yield_basis:{file:'yield-basis-ethereum',key:'yield-basis-create:1',name:'YieldBasisAdapter в Ethereum',load:()=>import('../assets/yield-basis-ethereum.json')},
    balancer_v1:{file:'balancer-v1-ethereum',key:'balancer-v1-create:1',name:'BalancerV1Adapter в Ethereum',load:()=>import('../assets/balancer-v1-ethereum.json')},
    stabull:stabullPlan, stabull_module:stabullPlan,
    rubicon_classic:{file:'rubicon-classic-base',key:'rubicon-classic-create:8453',name:'RubiconClassicWallet на Base'},
  };
  const carbonByChain = {
    '1':{file:'carbon-ethereum',key:'carbon-create:1',name:'CarbonWallet в Ethereum',load:()=>import('../assets/carbon-ethereum.json')},
    '8453':{file:'alien-carbon-base',key:'carbon-create:8453',name:'CarbonWallet на Base',load:()=>import('../assets/alien-carbon-base.json')},
  };
  const deployment = $derived(kind==='carbon_v1' ? carbonByChain[String(store.values['dex.evm_chain_id|Venue|'+venue])] : deployments[kind]);
  const recoverable = $derived(!!deployment?.load);
  const file = $derived(deployment?.file);
  let manifest = $state(null), estimateSeen = $state(null), pending = $state(null), txHash = $state('');
  let result = $state(''), error = $state(''), busy = $state(false), readyAddress = $state('');
  const key = $derived(deployment?.key);
  const value = (name) => store.values[name+'|Dex|'+id];
  const atVenue = (name) => store.values[name+'|Venue|'+venue];
  const current = () => ({chainId:atVenue('dex.evm_chain_id'), wrapped:atVenue('dex.native_wrapped'),
    factoryHash:value('dex.factory_runtime_hash'), template:value('dex.v1_template'),
    templateHash:value('dex.v1_template_runtime_hash'), cloneHash:value('dex.pair_runtime_hash'), kind:value('dex.kind'), market:value('dex.factory'),
    quoter:value('dex.quoter'), vault:value('dex.vault'), native:value('dex.bancor_native_token'), roles:value('dex.bancor_roles'),
    hashes:value('dex.module_runtime_hashes'), implementations:value('dex.proxy_implementations'),implementationHashes:value('dex.proxy_runtime_hashes')});
  let mounted = true;
  onDestroy(() => { mounted = false; });
  function scope() {
    return deploymentScope(localStorage, () => mounted && {
      id, venue, kind, key, settings:current(), router:value('dex.router'),
    });
  }
  function refreshPending(action) {
    pending = action.read();
    if (pending?.replacementHash || pending?.hash) txHash = pending.replacementHash ?? pending.hash;
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
    const action = scope(), plan = deployment;
    refreshPending(action);
    let artifact;
    if (plan.load) {
      artifact = (await plan.load()).default;
    } else {
      const response = await fetch(import.meta.env.BASE_URL+plan.file+'.json');
      if (!response.ok) throw new Error('Архив размещения отсутствует в этой сборке.');
      artifact = await response.json();
    }
    const prepared = await loadPlan(action.settings,artifact);
    action.check();
    manifest = prepared;
  }
  async function download() {
    const action = scope(), plan = manifest;
    await loadPlan(action.settings,plan);
    action.check();
    const url = URL.createObjectURL(new Blob([JSON.stringify(unsigned(plan), null, 2)], {type:'application/json'}));
    const a=document.createElement('a'); a.href=url; a.download=file+'-create.json'; a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
  async function price() {
    const action = scope(), plan = manifest;
    await loadPlan(action.settings, plan);
    action.check();
    const priced = await estimate(provider(), plan);
    action.check();
    estimateSeen = priced;
  }
  async function submit() {
    if (!navigator.locks) throw new Error('Для отправки из браузера требуется защищённое соединение с поддержкой Web Locks. Доступен экспорт.');
    const action = scope(), plan = manifest, expected = estimateSeen;
    await navigator.locks.request(action.key, {ifAvailable:true}, async (lock) => {
      action.check();
      refreshPending(action);
      if (!lock || pending || value('dex.router')) throw new Error('Есть ранее начатое размещение или настроенный модуль.');
      await loadPlan(action.settings, plan);
      action.check();
      try {
        const hash = await send(provider(), plan, expected, action.beforeOrAfterSend);
        if (action.active()) {
          txHash = hash;
          result = 'Транзакция передана кошельком. После финализации проверьте её по хешу.';
        }
      } finally {
        if (action.active()) refreshPending(action);
      }
    });
  }
  function sameIntent(a,b) {
    if (!a || !b) return a===b;
    return a.from===b.from && a.chain===b.chain && a.nonce===b.nonce && a.creation===b.creation;
  }
  async function reconcile(action,expected,act) {
    action.check();
    if (!recoverable) return act();
    const compare=async()=>{
      action.check();
      refreshPending(action);
      if (!sameIntent(pending,expected)) throw Error('Исходное поручение изменилось в другой вкладке. Повторите проверку.');
      return act();
    };
    if (navigator.locks) return navigator.locks.request(action.key,compare);
    return compare(); // Browsers without Web Locks can verify/export, but cannot submit a new request.
  }
  async function verify(action=scope()) {
    const supplied=txHash.trim(), plan=manifest;
    refreshPending(action);
    const original=pending;
    readyAddress='';
    await loadPlan(action.settings, plan);
    action.check();
    const fact = await deployed(provider(), plan, supplied, original);
    await reconcile(action,original,async()=>{
      if (fact.consumed) {
        localStorage.setItem(action.key+':lastResolution',JSON.stringify(fact));
        action.save(null); refreshPending(action); txHash=supplied; estimateSeen=null;
        result=fact.earlierCreation && fact.succeeded
          ? 'Финализированный CREATE относится к прежней версии модуля. Для действующей версии требуется отдельное размещение.'
          : 'Исходный nonce занят финализированной транзакцией. Размещение текущего модуля по этому поручению завершено без установки.';
      } else if (fact.reverted) {
        if (pending) { action.save(null); refreshPending(action); }
        txHash=supplied; estimateSeen=null;
        result='Финализированная транзакция завершилась откатом.';
      } else {
        readyAddress=fact.address;
        if (fact.replacementHash && pending) { action.save({...pending,replacementHash:fact.replacementHash}); refreshPending(action); }
        result=fact.replacementHash ? 'Замена CREATE с исходным nonce финализирована. Код модуля подтверждён.' : 'Каноническая транзакция и код модуля подтверждены.';
      }
    });
    return {original, address:fact.address};
  }
  async function useAddress() {
    const action=scope(), plan=manifest;
    const {original,address}=await verify(action);
    if (!address) throw new Error('Подтверждённого адреса модуля нет.');
    await reconcile(action,original,async()=>{
      await loadPlan(action.settings,plan);
      await action.install(address,setValue);
      action.save(null);
      if (!action.active()) return;
      refreshPending(action);
      result=kind === 'stabull' ? 'Адрес сохранён. Для защищённых обменов выберите вид stabull_module и сохраните настройки DEX.' : 'Подтверждённый адрес сохранён в dex.router.';
    });
  }

</script>

{#if deployment}
<details>
  <summary>Разместить {deployment.name}</summary>
  <button class="quiet" disabled={busy} onclick={() => run(prepare)}>Открыть параметры размещения</button>
  {#if manifest}
    {#if kind === 'balancer_v1' || kind === 'yield_basis'}
      <p>Фабрика {kind === 'yield_basis' ? 'YieldBasis' : 'BPool'} <code>{manifest.factory}</code>, WETH <code>{manifest.wrapped}</code>. При создании кошелёк отправляет 0 ETH.</p>
    {:else if kind === 'carbon_v1'}
      <p>Контроллер Carbon <code>{manifest.market}</code>. Реализация <code>{manifest.implementation}</code>. Сумма перевода при создании — 0 ETH.</p>
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
{/if}
