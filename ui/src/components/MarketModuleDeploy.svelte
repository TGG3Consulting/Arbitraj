<!-- Размещение принятого модуля одним CREATE в выбранной сети по действию оператора. -->
<script>
  import {unsignedCache} from '../lib/aquaDeploy.js';
  import {onDestroy} from 'svelte';
  import {deploymentScope} from '../lib/deploymentScope.js';
  import {store, setValue} from '../lib/settings.svelte.js';
  import {loadPlan, unsigned, estimate, send, deployed} from '../lib/walletDeployment.js';
  import {usesOriginalAktionariat} from '../lib/aktionariatDeploy.js';
  import ErrorText from './ErrorText.svelte';
  let {id, kind, venue} = $props();
  const aquaByChain = {
    '8453':{file:'aqua-base',key:'aqua-wallet:8453',name:'AquaStrategyWallet на Base',load:()=>import('../assets/aqua-base-create.json')},
    '1':{file:'aqua-ethereum',key:'aqua-wallet:1',name:'AquaStrategyWallet в Ethereum',load:()=>import('../assets/aqua-ethereum-create.json')},
  };
  const eulerByChain = {
    '1':{file:'eulerswap-ethereum',key:'eulerswap-create:1',name:'EulerSwapWallet в Ethereum',load:()=>import('../assets/eulerswap-ethereum.json')},
    '8453':{file:'eulerswap-base',key:'eulerswap-create:8453',name:'EulerSwapWallet на Base',load:()=>import('../assets/eulerswap-base.json')},
  };
  const stabullByChain = {
    '8453':{file:'stabull-base',key:'stabull-create:8453',name:'StabullAdapter на Base',load:()=>import('../assets/stabull-base.json')},
    '1':{file:'stabull-ethereum',key:'stabull-create:1',name:'StabullAdapter в Ethereum',load:()=>import('../assets/stabull-ethereum.json')},
  };
  const deployments = {
    algebra_integral_theorem:{file:'theorem-ethereum',key:'algebra-router-wallet:theorem:1',name:'AlgebraRouterWallet в Ethereum',load:()=>import('../assets/theorem-ethereum.json')},
    defiplaza_multi:{file:'defiplaza-multi',key:'defiplaza-multi:1',name:'DefiPlazaWallet для MultiPlaza',load:()=>import('../assets/defiplaza-multi-create.json')},
    defiplaza_stable:{file:'defiplaza-stable',key:'defiplaza-stable:1',name:'DefiPlazaWallet для StablePlaza',load:()=>import('../assets/defiplaza-stable-create.json')},
    llamma_lending:{file:'llamma-oneway',key:'llamma-oneway:1',name:'Curve LLAMMA lending в Ethereum',load:()=>import('../assets/llamma-oneway-create.json')},
    llamma_mint:{file:'llamma-mint',key:'llamma-mint:1',name:'Curve LLAMMA mint в Ethereum',load:()=>import('../assets/llamma-mint-create.json')},
    llamma_v2:{file:'llamma-v2',key:'llamma-v2:1',name:'Curve LLAMMA v2 в Ethereum',load:()=>import('../assets/llamma-v2-create.json')},
    ambient_ethereum:{file:'ambient-ethereum',key:'ambient-wallet:1',name:'AmbientWallet в Ethereum',load:()=>import('../assets/ambient-ethereum-create.json')},
    ammalgam_dlex:{file:'ammalgam-ethereum',key:'ammalgam-ethereum:1',name:'AmmalgamWallet в Ethereum',load:()=>import('../assets/ammalgam-ethereum-create.json')},
    mooniswap_v1:{file:'mooniswap-original',key:'mooniswap-original:1',name:'MooniswapWallet для Mooniswap',load:()=>import('../assets/mooniswap-original-create.json')},
    mooniswap_v2:{file:'mooniswap-successor',key:'mooniswap-successor:1',name:'MooniswapWallet для 1inch Liquidity Protocol',load:()=>import('../assets/mooniswap-successor-create.json')},
    shibaswap_v2_module:{file:'shibaswap-v2-ethereum',key:'shibaswap-v2-create:1',name:'ShibaswapV2Adapter в Ethereum',load:()=>import('../assets/shibaswap-v2-ethereum.json')},
    empire_module:{file:'empire-ethereum',key:'empire-create:1',name:'EmpireWallet в Ethereum',load:()=>import('../assets/empire-ethereum.json')},
    angstrom_l1:{file:'angstrom-ethereum',key:'angstrom-create:1',name:'AngstromWallet в Ethereum',load:()=>import('../assets/angstrom-ethereum.json')},
    uniswap_v1:{file:'uniswap-v1-ethereum',key:'uniswap-v1-create:1',name:'UniswapV1Adapter в Ethereum',load:()=>import('../assets/uniswap-v1-ethereum.json')},
    aktionariat:{file:'aktionariat-ethereum',key:'aktionariat-create:1',name:'AktionariatWallet в Ethereum',load:(settings)=>usesOriginalAktionariat(settings.hashes) ? import('../assets/aktionariat-ethereum-original.json') : import('../assets/aktionariat-ethereum.json')},
    bancor_legacy:{file:'bancor-v21-ethereum',key:'bancor-v21-create:1',name:'BancorLegacyAdapter в Ethereum',load:()=>import('../assets/bancor-v21-ethereum.json')},
    bancor_v3:{file:'bancor-v3-ethereum',key:'bancor-v3-create:1',name:'BancorV3Adapter в Ethereum',load:()=>import('../assets/bancor-v3-ethereum.json')},
    yield_basis:{file:'yield-basis-ethereum',key:'yield-basis-create:1',name:'YieldBasisAdapter в Ethereum',load:()=>import('../assets/yield-basis-ethereum.json')},
    balancer_v1:{file:'balancer-v1-ethereum',key:'balancer-v1-create:1',name:'BalancerV1Adapter в Ethereum',load:()=>import('../assets/balancer-v1-ethereum.json')},
    rubicon_classic:{file:'rubicon-classic-base',key:'rubicon-classic-create:8453',name:'RubiconClassicWallet на Base'},
  };
  const carbonByChain = {
    '1':{file:'carbon-ethereum',key:'carbon-create:1',name:'CarbonWallet в Ethereum',load:()=>import('../assets/carbon-ethereum.json')},
    '8453':{file:'alien-carbon-base',key:'carbon-create:8453',name:'CarbonWallet на Base',load:()=>import('../assets/alien-carbon-base.json')},
  };
  const communityPlans = {
    'goblins-base':{file:'goblins-community-base',key:'goblins-community-create:8453',name:'CommunityV3Adapter для Goblins',load:()=>import('../assets/goblins-community-base.json')},
    'wagmi-base':{file:'wagmi-community-base',key:'wagmi-community-create:8453',name:'CommunityV3Adapter для WAGMI',load:()=>import('../assets/wagmi-community-base.json')},
    'thick-base':{file:'thick-community-base',key:'thick-community-create:8453',name:'CommunityV3Adapter для Thick',load:()=>import('../assets/thick-community-base.json')},
  };
  const target = $derived(['registered_uni_v3','algebra_integral_theorem'].includes(kind) ? 'dex.v3_wallet_module' : 'dex.router');
  const deployment = $derived(kind==='registered_uni_v3'
    ? (String(store.values['dex.evm_chain_id|Venue|'+venue])==='8453' && value('dex.module_runtime_hashes')?.length ? communityPlans[id.split('@')[0]] : null)
    : /^eulerswap_v[12](?:_module)?$/.test(kind) ? eulerByChain[String(store.values['dex.evm_chain_id|Venue|'+venue])]
    : ['stabull','stabull_module'].includes(kind)
      ? stabullByChain[String(store.values['dex.evm_chain_id|Venue|'+venue])]
      : kind==='aqua_swapvm' ? aquaByChain[String(store.values['dex.evm_chain_id|Venue|'+venue])]
      : kind==='carbon_v1' ? carbonByChain[String(store.values['dex.evm_chain_id|Venue|'+venue])] : deployments[kind]);
  const recoverable = $derived(!!deployment?.load);
  const file = $derived(deployment?.file);
  let manifest = $state(null), estimateSeen = $state(null), pending = $state(null), txHash = $state('');
  let result = $state(''), error = $state(''), busy = $state(false), readyAddress = $state('');
  const key = $derived(deployment?.key);
  const value = (name) => store.values[name+'|Dex|'+id];
  const atVenue = (name) => store.values[name+'|Venue|'+venue];
  const current = () => ({chainId:atVenue('dex.evm_chain_id'), wrapped:atVenue('dex.native_wrapped'),
    officialRouter:value('dex.router'), routerHash:value('dex.router_runtime_hash'),deployer:value('dex.algebra_pool_deployer'),
    factoryHash:value('dex.factory_runtime_hash'), template:value('dex.v1_template'),
    templateHash:value('dex.v1_template_runtime_hash'), cloneHash:value('dex.pair_runtime_hash'), kind:value('dex.kind'), market:value('dex.factory'),
    ambientRoles:value('dex.ambient_roles'),ambientQuery:value('dex.ambient_query'),
    brokerMarkets:value('dex.broker_markets'),poolManager:value('dex.pool_manager'),launchRoles:value('dex.launch_roles'),
    quoter:value('dex.quoter'), quoterHash:value('dex.quoter_runtime_hash'), vault:value('dex.vault'), native:value('dex.bancor_native_token'), roles:value('dex.bancor_roles'),
    hashes:value('dex.module_runtime_hashes'), implementations:value('dex.proxy_implementations'),implementationHashes:value('dex.proxy_runtime_hashes')});
  let mounted = true;
  onDestroy(() => { mounted = false; });
  function scope() {
    return deploymentScope(localStorage, () => mounted && {
      id, venue, kind, key, settings:current(), router:value(target),
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
      artifact = (await plan.load(action.settings)).default;
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
    saveJson(unsigned(plan),file+'-create.json');
  }
  function saveJson(tx,name) {
    const url = URL.createObjectURL(new Blob([JSON.stringify(tx, null, 2)], {type:'application/json'}));
    const a=document.createElement('a'); a.href=url; a.download=name; a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
  async function cacheFile(entry) {
    const action=scope(), plan=manifest;
    await loadPlan(action.settings,plan);
    const tx=await unsignedCache(provider(),plan,value(target)?.toLowerCase(),entry);
    action.check();
    saveJson(tx,file+'-cache-'+entry.order+'.json');
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
      if (!lock || pending || value(target)) throw new Error('Есть ранее начатое размещение или настроенный модуль.');
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
      result=/^eulerswap_v[12]$/.test(kind) ? 'Адрес сохранён. Для защищённых обменов выберите '+kind+'_module и сохраните настройки DEX.' : kind === 'stabull' ? 'Адрес сохранён. Для защищённых обменов выберите вид stabull_module и сохраните настройки DEX.' : 'Подтверждённый адрес сохранён в '+target+'.';
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
    {:else if kind === 'algebra_integral_theorem'}
      <p>Фабрика <code>{manifest.factory}</code>, официальный Router <code>{manifest.router}</code>. После подтверждения адрес кошелька будет сохранён в <code>dex.v3_wallet_module</code>.</p>
    {:else if kind === 'registered_uni_v3'}
      <p>Фабрика Base <code>{manifest.factory}</code>. Подтверждённый адрес будет сохранён в <code>dex.v3_wallet_module</code>.</p>
    {:else if kind === 'empire_module'}
      <p>Фабрика Ethereum <code>{manifest.factory}</code>. При создании кошелёк отправляет 0 ETH.</p>
    {:else if kind === 'angstrom_l1'}
      <p>Конструктор принимает адаптер, хук и PoolManager Ethereum. Сумма перевода при создании — 0 ETH.</p>
      {#each ['adapter','hook','manager'] as role}<div>{role} <code>{manifest.roles[role].address}</code></div>{/each}
    {:else if kind === 'carbon_v1'}
      <p>Контроллер Carbon <code>{manifest.market}</code>. Реализация <code>{manifest.implementation}</code>. Сумма перевода при создании — 0 ETH.</p>
    {:else if /^eulerswap_v[12](?:_module)?$/.test(kind)}
      <p>Этот модуль принимает обе фабрики EulerSwap выбранной сети.</p>
      {#each manifest.factories as f, i}<div>Фабрика V{i+1} <code>{f.address}</code></div>{/each}
    {:else if ['stabull','stabull_module'].includes(kind)}
      <p>Конструктор принимает адрес фабрики <code>{manifest.market}</code>. Сумма перевода при создании — 0 ETH.</p>
    {:else if kind === 'aktionariat'}
      <p>Реестр акций <code>{manifest.registry}</code>. Дополнительных брокеров эмитентов: {manifest.issuerMarkets.length}. При создании кошелёк отправляет 0 ETH.</p>
    {:else if kind === 'bancor_legacy'}
      <p>Реестр конвертеров <code>{manifest.factory}</code>, BancorNetwork <code>{manifest.network}</code>, WETH <code>{manifest.wrapped}</code>. При создании кошелёк отправляет 0 ETH.</p>
    {:else if kind === 'bancor_v3'}
      <p>Конструктор Bancor V3 в Ethereum. Перевод ETH при создании — 0.</p>
      {#each ['BancorNetwork','BancorNetworkInfo','MasterVault','WETH','Нативный ETH'] as label, i}
        <div>{label} <code>{manifest.arguments[i]}</code></div>
      {/each}
    {:else if kind==='aqua_swapvm'}
      <p>SwapVM <code>{manifest.roles.engine.address}</code>, реестр Aqua <code>{manifest.roles.registry.address}</code>. После финализации адрес модуля сохраняется в <code>dex.router</code>.</p>
      {#if value(target)}
        <p>Прямой обмен принимает программу в calldata. Для V5 оператор один раз регистрирует программу вызовом cache на своём модуле.</p>
        {#each manifest.cache as entry}
          <p><code>{entry.order}</code> · {entry.gas} газа при архивной регистрации.</p>
          <button class="quiet" disabled={busy} onclick={()=>run(()=>cacheFile(entry))}>Скачать неподписанное поручение cache</button>
        {/each}
      {/if}
    {:else if kind==='ambient_ethereum'}
      <p>Контракт обмена <code>{manifest.exchange}</code>, данные пула <code>{manifest.query}</code>, WETH <code>{manifest.wrapped}</code>. После финализации модуль сохраняется в <code>dex.router</code>.</p>
    {:else if ['llamma_lending','llamma_mint','llamma_v2','ammalgam_dlex'].includes(kind)}
      <p>Фабрика Ethereum <code>{manifest.roles.factory.address}</code>. После финализации адрес модуля сохраняется в <code>dex.router</code>.</p>
    {:else if kind==='defiplaza_multi' || kind==='defiplaza_stable'}
      <p>Касса <code>{manifest.roles.factory.address}</code>, WETH <code>{manifest.roles.wrapped.address}</code>. Сумма CREATE составляет 0 ETH.</p>
      {:else if kind === 'uniswap_v1'}
      <p>Фабрика <code>{manifest.roles.factory.address}</code>, WETH <code>{manifest.roles.wrapped.address}</code>. Конструктор принимает оба адреса. Перевод ETH контракту — 0.</p>
    {:else}
      <p>Рынок <code>{manifest.market}</code>. Конструктор принимает этот адрес. Перевод ETH контракту — 0.</p>
    {/if}
    <p>Хеш проверенного runtime <code>{manifest.runtimeKeccak}</code>.</p>
    <button class="quiet" disabled={busy} onclick={() => run(download)}>Скачать неподписанный CREATE</button>
    {#if !pending && !value(target)}
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
      <button disabled={busy || !!value(target)} onclick={() => run(useAddress)}>Сохранить адрес модуля</button>
    {/if}
  {/if}
  {#if result}<p>{result}</p>{/if}
  <ErrorText text={error} />
</details>
{/if}
