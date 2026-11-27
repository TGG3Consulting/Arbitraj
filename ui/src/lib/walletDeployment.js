// Проверка неподписанного CREATE и канонического результата по принятому архиву модуля.
import {loadV1, checkV1, runtimeKeccak} from "./uniswapV1Deploy.js";
import {loadDexalot,checkDexalot} from './dexalotDeploy.js';
import {checkWalletOwner} from './registeredWalletModule.js';
import {isAirSwap,loadAirSwap,checkExchange} from './airswapDeploy.js';
import {isLlamma, loadLlamma, checkLlamma} from './llammaDeploy.js';
import {loadAmbient,checkAmbient} from './ambientDeploy.js';
import {loadAqua,checkAqua} from './aquaDeploy.js';
import {loadAmmalgam,checkAmmalgam} from './ammalgamDeploy.js';
import {mooniswap, loadMooniswap, checkCashRoles} from './mooniswapDeploy.js';
import {defiPlaza, loadDefiPlaza} from './defiplazaDeploy.js';
import {loadAktionariat,checkAktionariat} from './aktionariatDeploy.js';
import {loadAlgebra,checkAlgebra} from "./algebraWalletDeploy.js";
import {loadCarbon,checkCarbon} from './carbonDeploy.js';
import {loadFactoryWallet, checkFactoryWallet, supportsFactoryWallet} from './balancerV1Deploy.js';
import {singleFactory, loadSingleFactory, checkSingleFactory} from "./singleFactoryDeploy.js";
import {loadAngstrom, checkAngstrom} from "./angstromDeploy.js";
import {loadStabull, checkStabull} from './stabullDeploy.js';
import {loadBancor, checkBancor} from "./bancorDeploy.js";
const implementationSlot = '0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc';
const ownedModule = kind => isAirSwap(kind) || kind==='dexalot_rfq';
const bindsWallet = kind => kind==='aqua_swapvm' || ownedModule(kind) || isLlamma(kind) || kind==='ambient_ethereum' || kind==='ammalgam_dlex' || defiPlaza(kind) || mooniswap(kind) || singleFactory(kind) || supportsFactoryWallet(kind) || ['aktionariat','algebra_integral_theorem','angstrom_l1','uniswap_v1','bancor_v3','stabull','carbon_v1'].includes(kind);
const address = /^0x[\da-f]{40}$/i;
const digest = /^0x[\da-f]{64}$/i;

function requireFact(ok, message) {
  if (!ok) throw new Error(message);
}

export async function hashCode(hex) {
  requireFact(/^0x(?:[\da-f]{2})+$/i.test(hex), 'В ответе отсутствует байткод контракта.');
  const data = Uint8Array.from(hex.slice(2).match(/../g), (s) => parseInt(s, 16));
  const sha = await crypto.subtle.digest('SHA-256', data);
  return Array.from(new Uint8Array(sha), (n) => n.toString(16).padStart(2, '0')).join('');
}

export async function loadPlan(settings, plan) {
  if (settings.kind==='aqua_swapvm' || plan.kind==='aqua_swapvm') return loadAqua(settings,plan,hashCode);
  if (settings.kind==='dexalot_rfq' || plan.kind==='dexalot_rfq') return loadDexalot(settings,plan,hashCode);
  if (isAirSwap(settings.kind) || isAirSwap(plan.kind)) return loadAirSwap(settings,plan,hashCode);
  if (isLlamma(settings.kind) || isLlamma(plan.kind)) return loadLlamma(settings,plan,hashCode);
  if (settings.kind==='ambient_ethereum' || plan.kind==='ambient_ethereum') return loadAmbient(settings,plan,hashCode);
  if (settings.kind==='ammalgam_dlex' || plan.kind==='ammalgam_dlex') return loadAmmalgam(settings,plan,hashCode);
  if (defiPlaza(settings.kind) || defiPlaza(plan.kind)) return loadDefiPlaza(settings,plan,hashCode);
  if (mooniswap(settings.kind) || mooniswap(plan.kind)) return loadMooniswap(settings,plan,hashCode);
  if (settings.kind==='aktionariat' || plan.kind==='aktionariat') return loadAktionariat(settings,plan,hashCode);
  if (settings.kind==='algebra_integral_theorem' || plan.kind==='algebra_integral_theorem') return loadAlgebra(settings,plan,hashCode);
  if (singleFactory(settings.kind) || singleFactory(plan.kind)) return loadSingleFactory(settings,plan,hashCode);
  if (settings.kind==='angstrom_l1' || plan.kind==='angstrom_l1') return loadAngstrom(settings,plan,hashCode);
  if (settings.kind==='carbon_v1' || plan.kind==='carbon_v1') return loadCarbon(settings,plan,hashCode);
  if (supportsFactoryWallet(settings.kind) || supportsFactoryWallet(plan.kind)) return loadFactoryWallet(settings,plan,hashCode);
  if (['stabull','stabull_module'].includes(settings.kind) || plan.kind === 'stabull') return loadStabull(settings, plan, hashCode);
  if (settings.kind === "bancor_v3" || plan.kind === "bancor_v3") return loadBancor(settings,plan,hashCode);
  if (settings.kind === "uniswap_v1" || plan.kind === "uniswap_v1") return loadV1(settings,plan,hashCode);
  requireFact(plan.chainId === 8453 && address.test(plan.market) && address.test(plan.implementation)
    && digest.test(plan.runtimeKeccak), 'Формат архивного размещения не распознан.');
  requireFact(settings.kind === 'rubicon_classic' && settings.market?.toLowerCase() === plan.market
    && settings.hashes?.some((s) => s.toLowerCase() === plan.runtimeKeccak)
    && settings.implementations?.some((s) => s.toLowerCase() === plan.implementation),
    'Фабрика, реализация или допустимый код модуля в настройках отличаются от проверенного размещения.');
  requireFact(await hashCode(plan.data) === plan.creationSha256
    && plan.data.toLowerCase().endsWith(plan.market.slice(2).padStart(64, '0')), 'Байткод создания или аргумент конструктора изменился.');
  return plan;
}

const call = (provider, method, params = []) => provider.request({method, params});

async function checkChain(provider, p) {
  requireFact(BigInt(await call(provider, 'eth_chainId')) === BigInt(p.chainId), 'Выберите '+(p.chainId === 1 ? 'Ethereum' : 'Base')+' в кошельке.');
}

export async function checkMarket(provider, p, at) {
  await checkChain(provider, p);
  if (p.kind==='aqua_swapvm') return checkAqua(provider,p,at);
  if (p.kind==='dexalot_rfq') return checkDexalot(provider,p,at);
  if (isAirSwap(p.kind)) return checkExchange(provider,p,at);
  if (isLlamma(p.kind)) return checkLlamma(provider,p,at);
  if (p.kind==='ambient_ethereum') return checkAmbient(provider,p,at);
  if (p.kind==='ammalgam_dlex') return checkAmmalgam(provider,p,at);
  if (mooniswap(p.kind) || defiPlaza(p.kind)) return checkCashRoles(provider,p,at);
  if (p.kind==='aktionariat') return checkAktionariat(provider,p,at);
  if (p.kind==='algebra_integral_theorem') return checkAlgebra(provider,p,at);
  if (singleFactory(p.kind)) return checkSingleFactory(provider,p,at);
  if (p.kind==='angstrom_l1') return checkAngstrom(provider,p,at);
  if (p.kind==='carbon_v1') return checkCarbon(provider,p,at);
  if (supportsFactoryWallet(p.kind)) return checkFactoryWallet(provider,p,at);
  if (p.kind === 'stabull') return checkStabull(provider, p, at);
  if (p.kind === "bancor_v3") return checkBancor(provider,p,at);
  if (p.kind === "uniswap_v1") return checkV1(provider,p,at,hashCode);
  const [proxy, slot, code] = await Promise.all([
    call(provider, 'eth_getCode', [p.market, at]),
    call(provider, 'eth_getStorageAt', [p.market, implementationSlot, at]),
    call(provider, 'eth_getCode', [p.implementation, at]),
  ]);
  requireFact(digest.test(slot) && slot.toLowerCase().slice(2) === p.implementation.slice(2).padStart(64, '0')
    && await hashCode(proxy) === p.proxySha256 && await hashCode(code) === p.implementationSha256,
    'Код рынка или активная реализация изменились.');
}

async function sameRuntime(code, p) {
  return await hashCode(code) === p.runtimeSha256 && (!bindsWallet(p.kind) || runtimeKeccak(code) === p.runtimeKeccak);
}

export function unsigned(p) {
  return {chainId: '0x'+p.chainId.toString(16), data: p.data, value: '0x0'};
}

export async function estimate(provider, p) {
  await checkChain(provider, p);
  const accounts = await call(provider, 'eth_requestAccounts');
  requireFact(address.test(accounts?.[0]), 'Кошелёк не предоставил адрес плательщика.');
  const from = accounts[0].toLowerCase();
  if (ownedModule(p.kind)) requireFact(from===p.wallet,'Выберите в браузере кошелёк, для которого размещается модуль.');
  const head = await call(provider, 'eth_getBlockByNumber', ['latest', false]);
  const at = {blockHash:head.hash, requireCanonical:true};
  await checkMarket(provider, p, at);
  const tx = {...unsigned(p), from};
  const [gas, price, runtime] = await Promise.all([
    call(provider, 'eth_estimateGas', [tx]), call(provider, 'eth_gasPrice'), call(provider, 'eth_call', [tx, at]),
  ]);
  requireFact(await sameRuntime(runtime,p), 'Проверка создания вернула другой код модуля.');
  return {from, gas:BigInt(gas).toString(), fee:(BigInt(gas)*BigInt(price)).toString()};
}

export async function send(provider, p, expected, save) {
  const actual = await estimate(provider, p);
  requireFact(actual.from === expected.from && BigInt(actual.fee) <= BigInt(expected.fee),
    'Изменилась оценка газа или плательщик. Обновите расчёт.');
  const nonce = await call(provider, 'eth_getTransactionCount', [expected.from, 'pending']);
  await checkChain(provider, p);
  const tx = {...unsigned(p), from:expected.from, nonce};
  const pending = {chain:p.chainId, from:expected.from, nonce, creation:p.creationSha256};
  save(pending);
  try {
    const hash = await call(provider, 'eth_sendTransaction', [tx]);
    requireFact(digest.test(hash), 'Кошелёк не вернул хеш. Проверьте отправку по исходному nonce.');
    save({...pending, hash});
    return hash;
  } catch (e) {
    if (e.code === 4001) save(null);
    throw e;
  }
}

export async function deployed(provider, p, hash, pending) {
  requireFact(digest.test(hash), 'Введите полный хеш транзакции создания.');
  await checkChain(provider, p);
  const [tx, r] = await Promise.all([
    call(provider, 'eth_getTransactionByHash', [hash]), call(provider, 'eth_getTransactionReceipt', [hash]),
  ]);
  requireFact(tx && r && r.transactionHash?.toLowerCase()===hash.toLowerCase(), 'Каноническая квитанция пока не найдена.');
  const resolveReplacement = bindsWallet(p.kind);
  if (ownedModule(p.kind)) requireFact(tx.from?.toLowerCase()===p.wallet,'Эту транзакцию подписал другой кошелёк.');
  const requested = tx.to == null && BigInt(tx.value) === 0n && tx.input?.toLowerCase() === p.data.toLowerCase();
  requireFact(tx.hash.toLowerCase() === hash.toLowerCase() && tx.blockHash === r.blockHash &&
    (requested || (resolveReplacement && pending)), 'Транзакция не создаёт этот модуль.');
  if (tx.chainId != null) requireFact(BigInt(tx.chainId) === BigInt(p.chainId), 'Транзакция принадлежит другой сети.');
  if (pending) requireFact(tx.from.toLowerCase() === pending.from && BigInt(tx.nonce) === BigInt(pending.nonce) &&
    pending.chain === p.chainId && (resolveReplacement || pending.creation === p.creationSha256),
    'Плательщик, nonce или исходное поручение отличаются.');
  if (resolveReplacement) {
    const who = pending?.from ?? (await call(provider, 'eth_requestAccounts'))?.[0];
    requireFact(address.test(who) && who.toLowerCase() === tx.from.toLowerCase(), 'Плательщик канонического CREATE отличается от кошелька.');
  }
  const final = await call(provider, 'eth_getBlockByNumber', ['finalized', false]);
  const canonical = await call(provider, 'eth_getBlockByNumber', [r.blockNumber, false]);
  requireFact(final && canonical?.hash === r.blockHash && BigInt(final.number) >= BigInt(r.blockNumber),
    'Транзакция ожидает финализированного блока.');
  const changed = resolveReplacement && pending && (pending.creation !== p.creationSha256 || !requested ||
    (pending.hash && pending.hash.toLowerCase() !== hash.toLowerCase()));
  if (changed) {
    requireFact([0n,1n].includes(BigInt(r.status)) && /^[a-f0-9]{64}$/i.test(pending.creation) &&
      canonical.transactions?.some(h => typeof h === 'string' && h.toLowerCase() === hash.toLowerCase()),
      'Канонический блок не подтверждает включение замены с исходным nonce.');
    if (!requested || pending.creation !== p.creationSha256) {
      const earlier = tx.to == null && BigInt(tx.value) === 0n && /^0x(?:[0-9a-f]{2})+$/i.test(tx.input) &&
        await hashCode(tx.input) === pending.creation;
      return {consumed:true, earlierCreation:earlier, succeeded:BigInt(r.status)===1n, hash,
        originalHash:pending.hash ?? null, blockHash:r.blockHash, from:pending.from, nonce:pending.nonce};
    }
  }
  if (BigInt(r.status) === 0n) return {reverted:true};
  requireFact(BigInt(r.status) === 1n && address.test(r.contractAddress), 'Квитанция не подтверждает создание контракта.');
  const at = {blockHash:final.hash, requireCanonical:true};
  await checkMarket(provider, p, at);
  if (bindsWallet(p.kind)) await checkMarket(provider, p, {blockHash:r.blockHash,requireCanonical:true});
  const code = await call(provider, 'eth_getCode', [r.contractAddress, at]);
  requireFact(await sameRuntime(code,p), 'Размещённый runtime отличается от проверенного кода.');
  if (ownedModule(p.kind)) await checkWalletOwner(provider,p,r.contractAddress,at);
  return changed ? {address:r.contractAddress.toLowerCase(),replacementHash:hash} : {address:r.contractAddress.toLowerCase()};
}
