<!-- Размещение модуля, привязанного к одному кошельку и поколению AirSwap. -->
<script>
  import {onDestroy} from 'svelte';
  import {deploymentScope} from '../lib/deploymentScope.js';
  import {setValue,store} from '../lib/settings.svelte.js';
  import {loadPlan,estimate,send,deployed} from '../lib/walletDeployment.js';
  import {addModule} from '../lib/airswapDeploy.js';

  let {venue,wallet,chain,version,load}=$props();
  const scope={kind:'Venue',get id(){return venue;}};
  const rows=$derived(store.values['airswap.contracts|Venue|'+venue] ?? []);
  const storageKey=$derived(['airswap-create',venue,chain,version,wallet.toLowerCase()].join(':'));
  let busy=$state(false),error=$state(''),message=$state(''),priced=$state(null),hash=$state('');
  let mounted=true;
  onDestroy(()=>{mounted=false;});
  const current=()=>mounted && {id:wallet.toLowerCase(),venue,kind:'airswap_'+version,key:storageKey,
    settings:{kind:'airswap_'+version,wallet,chainId:chain,rows},router:null};
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
  async function price(){
    const {a,p}=await context();
    if (a.read()) throw Error('Уже есть отправка. Проверьте исходный хеш или замену с тем же nonce.');
    const value=await estimate(provider(),p);a.check();priced=value;
  }
  async function submit(){
    const {a,p}=await context(), budget=priced;
    if (!navigator.locks || !budget) throw Error('Нужны защищённое соединение и оценка газа.');
    await navigator.locks.request(a.key,{ifAvailable:true},async lock=>{
      a.check();
      if (!lock || a.read()) throw Error('Размещение уже начато.');
      const h=await send(provider(),p,budget,a.beforeOrAfterSend);
      if(a.active()){hash=h;priced=null;message='Транзакция отправлена. После финализации проверьте хеш.';}
    });
  }
  async function verify(){
    const input=hash.trim(),{a,p}=await context(),original=a.read();
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
      if (next!==a.settings.rows) await setValue('airswap.contracts',scope,next,a.settings.rows);
      a.save(null);priced=null;message='Модуль '+fact.address+' сохранён для '+p.wallet+'.';
    };
    if(navigator.locks) await navigator.locks.request(a.key,install); else await install();
  }
</script>
<details ontoggle={(e)=>{if(e.currentTarget.open) reopen();}}>
  <summary>Модуль AirSwap {version.toUpperCase()}</summary>
  <p>Плательщик и получатель <code>{wallet}</code>. Сеть {chain}. Модуль проверяет фактическую выплату по подписанной цене.</p>
  <button disabled={busy} onclick={()=>run(price)}>Оценить газ</button>
  {#if priced}
    <p>{priced.gas} gas · до {priced.fee} wei</p>
    <button disabled={busy} onclick={()=>run(submit)}>Развернуть через кошелёк</button>
  {/if}
  <label>Хеш создания или его замены <input bind:value={hash} placeholder="0x…" /></label>
  <button disabled={busy || !/^0x[0-9a-f]{64}$/i.test(hash.trim())} onclick={()=>run(verify)}>Проверить и сохранить</button>
  {#if message}<p role="status">{message}</p>{/if}
  {#if error}<p class="err" role="alert">{error}</p>{/if}
</details>
