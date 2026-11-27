// Проверка неподписанного CREATE и канонического результата для Rubicon Classic.
const implementationSlot = '0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc';
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
  requireFact(BigInt(await call(provider, 'eth_chainId')) === BigInt(p.chainId), 'Выберите Base в кошельке.');
}

export async function checkMarket(provider, p, at) {
  await checkChain(provider, p);
  const [proxy, slot, code] = await Promise.all([
    call(provider, 'eth_getCode', [p.market, at]),
    call(provider, 'eth_getStorageAt', [p.market, implementationSlot, at]),
    call(provider, 'eth_getCode', [p.implementation, at]),
  ]);
  requireFact(digest.test(slot) && slot.toLowerCase().slice(2) === p.implementation.slice(2).padStart(64, '0')
    && await hashCode(proxy) === p.proxySha256 && await hashCode(code) === p.implementationSha256,
    'Код рынка или активная реализация изменились.');
}

export function unsigned(p) {
  return {chainId: '0x'+p.chainId.toString(16), data: p.data, value: '0x0'};
}

export async function estimate(provider, p) {
  await checkChain(provider, p);
  const accounts = await call(provider, 'eth_requestAccounts');
  requireFact(address.test(accounts?.[0]), 'Кошелёк не предоставил адрес плательщика.');
  const from = accounts[0].toLowerCase();
  const head = await call(provider, 'eth_getBlockByNumber', ['latest', false]);
  const at = {blockHash:head.hash, requireCanonical:true};
  await checkMarket(provider, p, at);
  const tx = {...unsigned(p), from};
  const [gas, price, runtime] = await Promise.all([
    call(provider, 'eth_estimateGas', [tx]), call(provider, 'eth_gasPrice'), call(provider, 'eth_call', [tx, at]),
  ]);
  requireFact(await hashCode(runtime) === p.runtimeSha256, 'Проверка создания вернула другой код модуля.');
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
  requireFact(tx.hash.toLowerCase() === hash.toLowerCase() && tx.to == null && BigInt(tx.value) === 0n
    && tx.input.toLowerCase() === p.data.toLowerCase() && tx.blockHash === r.blockHash, 'Транзакция не создаёт этот модуль.');
  if (tx.chainId != null) requireFact(BigInt(tx.chainId) === BigInt(p.chainId), 'Транзакция принадлежит другой сети.');
  if (pending) requireFact(tx.from.toLowerCase() === pending.from && BigInt(tx.nonce) === BigInt(pending.nonce)
    && pending.creation === p.creationSha256 && pending.chain === p.chainId, 'Плательщик, nonce или исходное поручение отличаются.');
  const final = await call(provider, 'eth_getBlockByNumber', ['finalized', false]);
  const canonical = await call(provider, 'eth_getBlockByNumber', [r.blockNumber, false]);
  requireFact(final && canonical?.hash === r.blockHash && BigInt(final.number) >= BigInt(r.blockNumber),
    'Транзакция ожидает финализированного блока Base.');
  if (BigInt(r.status) === 0n) return {reverted:true};
  requireFact(BigInt(r.status) === 1n && address.test(r.contractAddress), 'Квитанция не подтверждает создание контракта.');
  const at = {blockHash:final.hash, requireCanonical:true};
  await checkMarket(provider, p, at);
  const code = await call(provider, 'eth_getCode', [r.contractAddress, at]);
  requireFact(await hashCode(code) === p.runtimeSha256, 'Размещённый runtime отличается от проверенного кода.');
  return {address:r.contractAddress.toLowerCase()};
}
