<!-- Размещение модуля, для выбранной сети и владельца. -->
<script>
  import {onDestroy} from 'svelte';
  import {deploymentScope,sourceDeploymentKey} from '../lib/deploymentScope.js';
  import {setValue,store} from '../lib/settings.svelte.js';
  import {loadPlan,estimate,send,deployed} from '../lib/walletDeployment.js';

  let {venue,wallet,chain,version,kind,source,label,setting,load,recoverPlan,addModule,registeredOwnerModule,shared=false,wrapped,book,bookModule}=$props();
  const rows=$derived(store.values[setting+'|Venue|'+venue] ?? (shared ? null : []));
  const configured=$derived(book ? bookModule : (shared ? rows : null));
  const storageKey=$derived(sourceDeploymentKey(source,venue,chain,version,wallet,shared));
  let busy=$state(false),error=$state(''),message=$state(''),priced=$state(null),hash=$state('');
  let mounted=true;
  onDestroy(()=>{mounted=false;});
  const current=()=>mounted && {id:wallet.toLowerCase(),venue,kind,key:storageKey,
    settings:{kind,wallet,chainId:chain,rows,wrapped,book},router:configured};
  const action=()=>deploymentScope(localStorage,current);
  const provider=()=>{
    if (!window.ethereum?.request) throw Error('Подключите кошелёк в браузере.');
    return window.ethereum;
  };
  async function run(fn){
    if (busy) return;
    busy=true;error='';message='';
    try { await fn(); } catch(e){error=e.message;} finally {busy=false;}
  }
  async function context(){
    const a=action(), p=await loadPlan(a.settings,(await load()).default);
    a.check();
    return {a,p};
  }
  function pending(){return action().read();}
  function reopen(){
    const record=pending();
    if (record?.hash) hash=record.hash;
  }
  async function alreadyRegistered(a,p){
    const existing=await registeredOwnerModule(provider(),p,a.settings.rows);
    a.check();
    if (existing){priced=null;message=(shared?'Общий модуль этой сети уже настроен: ':'Для этого кошелька уже зарегистрирован модуль ')+existing+'.';}
    return Boolean(existing);
  }
  async function price(){
    const {a,p}=await context();
    if (a.read()) throw Error('Уже есть отправка. Проверьте исходный хеш или замену с тем же nonce.');
    if (await alreadyRegistered(a,p)) return;
    const value=await estimate(provider(),p);a.check();priced=value;
  }
  async function submit(){
    const {a,p}=await context(), budget=priced;
    if (!navigator.locks || !budget) throw Error('Нужны защищённое соединение и оценка газа.');
    await navigator.locks.request(a.key,{ifAvailable:true},async lock=>{
      a.check();
      if (!lock || a.read()) throw Error('Размещение уже начато.');
      if (await alreadyRegistered(a,p)) return;
      const h=await send(provider(),p,budget,a.beforeOrAfterSend);
      if(a.active()){hash=h;priced=null;message='Транзакция отправлена. После финализации проверьте хеш.';}
    });
  }
  async function verify(){
    const input=hash.trim(),{a,p:currentPlan}=await context(),original=a.read();
    const p=recoverPlan ? await recoverPlan(provider(),a.settings,currentPlan,original,input) : currentPlan;
    a.check();
    const fact=await deployed(provider(),p,input,original);
    const install=async()=>{
      a.check();
      if (JSON.stringify(original)!==JSON.stringify(a.read())) throw Error('Поручение изменилось в другой вкладке.');
      if (fact.consumed || fact.reverted){
        a.save(null);priced=null;
        message=fact.reverted?'Транзакция создания откатилась.':'Исходный nonce занят финализированной заменой.';
        return;
      }
      const next=addModule(a.settings.rows,p,fact.address);
      await a.installSource(setting,next,setValue);
      a.save(null);priced=null;message='Модуль '+fact.address+' сохранён для '+(shared?venue:p.wallet)+'.';
    };
    if(navigator.locks) await navigator.locks.request(a.key,install); else await install();
  }
</script>
<details ontoggle={(e)=>{if(e.currentTarget.open) reopen();}}>
  <summary>Модуль {label}</summary>
  {#if shared}
    <p>Общий исполнитель для кошельков площадки <code>{venue}</code>. Плательщик размещения <code>{wallet}</code>.
      Сеть {chain}. {#if book}Книга <code>{book}</code>.{:else}WETH <code>{wrapped ?? 'не задан'}</code>.{/if}</p>
    {#if configured}<p>Заданный адрес <code>{configured}</code></p>{/if}
  {:else}
    <p>Плательщик и получатель <code>{wallet}</code>. Сеть {chain}. Модуль проверяет фактическую выплату по подписанной цене.</p>
  {/if}
  <button disabled={busy} onclick={()=>run(price)}>{configured ? 'Проверить настроенный адрес' : 'Оценить газ'}</button>
  {#if priced}
    {#if shared}
      <p>Исполнение на Base потребует примерно {priced.gas} gas ({priced.fee} wei). Перед подписью проверьте также плату за публикацию в L1.</p>
    {:else}
      <p>{priced.gas} gas · до {priced.fee} wei</p>
    {/if}
    <button disabled={busy} onclick={()=>run(submit)}>Развернуть через кошелёк</button>
  {/if}
  <label>Хеш создания или его замены <input bind:value={hash} placeholder="0x…" /></label>
  <button disabled={busy || !/^0x[0-9a-f]{64}$/i.test(hash.trim())} onclick={()=>run(verify)}>Проверить и сохранить</button>
  {#if message}<p role="status">{message}</p>{/if}
  {#if error}<p class="err" role="alert">{error}</p>{/if}
</details>
