// Проверяет CREATE и формирует неподписанное поручение cache для конкретного модуля и программы.
import {runtimeKeccak} from './uniswapV1Deploy.js';

const word = s => s.slice(2).padStart(64,'0');
const address = /^0x[0-9a-f]{40}$/;
const digest = /^0x[0-9a-f]{64}$/;
const same = (s, v) => typeof s === 'string' && s.toLowerCase() === v;

export async function loadAqua(settings, plan, sha) {
  if (plan.kind !== 'aqua_swapvm' || settings.kind !== plan.kind || ![1,8453].includes(plan.chainId)
      || String(plan.chainId) !== String(settings.chainId) || !digest.test(plan.sourceBlock)
      || !settings.hashes?.some(h => same(h,plan.runtimeKeccak)) || !digest.test(plan.v5Code)) {
    throw Error('Сеть, вид DEX или хеш модуля Aqua расходится с архивом.');
  }
  const {engine,registry} = plan.roles;
  for (const [role,addr,hash] of [[engine,settings.quoter,settings.quoterHash],[registry,settings.market,settings.factoryHash]]) {
    if (!address.test(role.address) || !same(addr,role.address) || !digest.test(role.keccak256) || !same(hash,role.keccak256)) {
      throw Error('Адрес или код SwapVM и Aqua расходится с выбранным профилем.');
    }
  }
  if (engine.address === registry.address || !plan.data.endsWith(word(engine.address)+word(registry.address)+word(plan.v5Code))
      || await sha(plan.data) !== plan.creationSha256 || !/^[0-9a-f]{64}$/.test(plan.runtimeSha256)) {
    throw Error('Изменились конструктор или байткод AquaStrategyWallet.');
  }
  return plan;
}

export async function checkAqua(provider, plan, at) {
  if (!at?.requireCanonical || !digest.test(at.blockHash)) throw Error('Для проверки Aqua нужен канонический блок.');
  const {engine,registry} = plan.roles;
  for (const role of [engine,registry]) {
    const code = await provider.request({method:'eth_getCode',params:[role.address,at]});
    if (runtimeKeccak(code) !== role.keccak256) throw Error('Изменился код '+role.address+'.');
  }
  const actual=await provider.request({method:'eth_call',params:[{to:engine.address,data:'0x63fccba9'},at]});
  if (!same(actual,'0x'+word(registry.address))) throw Error('SwapVM использует другой реестр Aqua.');
}

export async function unsignedCache(provider, plan, target, entry) {
  if (!address.test(target) || !digest.test(entry.order) || !entry.data.startsWith('0xa3712253')
      || runtimeKeccak('0x'+entry.data.slice(10)) !== entry.order || !plan.cache.includes(entry)) {
    throw Error('Адрес модуля или полная программа cache не совпадают с архивом.');
  }
  const rpc = (method, params=[]) => provider.request({method,params});
  if (BigInt(await rpc('eth_chainId')) !== BigInt(plan.chainId)) throw Error('Выберите сеть из профиля Aqua.');
  const head=await rpc('eth_getBlockByNumber',['latest',false]);
  const at={blockHash:head.hash,requireCanonical:true};
  await checkAqua(provider,plan,at);
  if (runtimeKeccak(await rpc('eth_getCode',[target,at])) !== plan.runtimeKeccak) throw Error('Код модуля для cache изменился.');
  const {engine,registry}=plan.roles;
  const counts=await Promise.all(entry.tokens.map(async token=>{
    const raw=await rpc('eth_call',[{to:registry.address,data:'0x6d58b4cc'+word(entry.maker)+word(engine.address)+word(entry.order)+word(token)},at]);
    if (!/^0x[0-9a-f]{128}$/i.test(raw)) throw Error('Не удалось прочитать регистрацию токена Aqua.');
    return BigInt('0x'+raw.slice(66));
  }));
  if (counts.filter(n=>n>0n && n<255n).length<2) throw Error('Для программы зарегистрировано меньше двух текущих валют.');
  const key=runtimeKeccak('0x'+word(entry.order)+'0'.repeat(64));
  const cached=await rpc('eth_call',[{to:target,data:'0xde991b29'+word(key)},at]);
  if (same(cached,entry.order)) throw Error('Оба порядка оплаты уже зарегистрированы. Повторная оплата газа не требуется.');
  if (!same(cached,'0x'+'0'.repeat(64))) throw Error('Сохранённый ключ указывает на другую программу.');
  const tx={chainId:'0x'+plan.chainId.toString(16),to:target,data:entry.data,value:'0x0'};
  if (!same(await rpc('eth_call',[{to:target,data:entry.data},at]),entry.order)) throw Error('Регистрация программы не подтверждается симуляцией.');
  return tx;
}
