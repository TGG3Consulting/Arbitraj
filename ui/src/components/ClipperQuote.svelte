<!-- Индивидуальная проверка подписи ClipperDirect и реальных средств выбранного кошелька. -->
<script>
  import { post, seg } from '../lib/api.js';
  let {venue, trader, wallet} = $props();
  let input=$state(''), output=$state(''), amount=$state(''), minimum=$state(''), pool=$state('');
  let busy=$state(false), error=$state(''), result=$state(null);
  const addr=/^0x[0-9a-f]{40}$/i, raw=/^[1-9]\d*$/;
  const valid=$derived(addr.test(input)&&addr.test(output)&&input.toLowerCase()!==output.toLowerCase()&&raw.test(amount)&&raw.test(minimum)&&(!pool||addr.test(pool)));
  let query=0;
  $effect(()=>{void venue;void trader;void wallet;void input;void output;void amount;void minimum;void pool;query++;busy=false;result=null;error='';});
  async function ask(e){
    e.preventDefault();if(!valid||busy)return;
    const current=query;busy=true;result=null;error='';
    try{
      const row=await post(`/api/venues/${seg(venue)}/sources/clipper/quote`,{input,output,amount_raw:amount,trader,market:pool||null,firm:{wallet,minimum_raw:minimum}});
      if(current===query)result=row;
    }catch(e){if(current===query)error=e.message;}
    finally{if(current===query)busy=false;}
  }
</script>
<details>
  <summary>Проверить котировку Clipper / Blade</summary>
  <form onsubmit={ask}>
    <label>Адрес входного актива <input bind:value={input} placeholder="0x…" /></label>
    <label>Адрес выходного актива <input bind:value={output} placeholder="0x…" /></label>
    <p>Для ETH укажите нулевой адрес. Для WETH укажите адрес токена.</p>
    <label>Расход в минимальных единицах <input bind:value={amount} inputmode="numeric" /></label>
    <label>Минимум зачисления в единицах выхода <input bind:value={minimum} inputmode="numeric" /></label>
    <label>Адрес пула, если в профиле несколько <input bind:value={pool} placeholder="0x…" /></label>
    <button disabled={!valid||busy}>{busy?'Запрос…':'Проверить подпись и оплату в симуляции'}</button>
  </form>
  {#if error}<p class="err" role="status">{error}</p>{/if}
  {#if result}<p role="status">В симуляции блока после {result.block} кошелёк израсходовал {result.spent_raw} единиц входа и получил {result.received_raw} единиц выхода. Газ обмена — {result.details.gas_used}. Срок предложения — {new Date(result.expires*1000).toLocaleTimeString()}.</p>{/if}
</details>
<style>
  form { display: grid; gap: .5rem; max-width: 34rem; margin-block: .5rem; }
  label { display: grid; gap: .2rem; }
  button { justify-self: start; }
</style>
