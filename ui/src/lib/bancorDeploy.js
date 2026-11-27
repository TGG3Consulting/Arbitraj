import {runtimeKeccak} from "./uniswapV1Deploy.js";

const word = (address) => address.slice(2).padStart(64, '0');
const same = (configured, actual) => typeof configured === 'string' && configured.toLowerCase() === actual;
const address = /^0x[0-9a-f]{40}$/;
const digest = /^0x[0-9a-f]{64}$/;
const names = ['BancorNetwork', 'BancorNetworkInfo', 'MasterVault', 'BNTPool', 'NetworkSettings'];
const slot = '0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc';

export async function loadBancor(settings, plan, hashCode) {
  const args = plan.arguments;
  const roles = names.map(name => plan.roles?.[name]);
  if (plan.kind !== 'bancor_v3' || plan.chainId !== 1 || !Array.isArray(args) || args.length !== 5 ||
      new Set(args).size !== 5 || !args.every(a => address.test(a) && !/^0x0+$/.test(a)) ||
      !roles.every(r => r && address.test(r.address) && digest.test(r.keccak256) &&
        address.test(r.implementation?.address) && digest.test(r.implementation?.keccak256)) ||
      !digest.test(plan.runtimeKeccak) || !digest.test(plan.wrappedHash) ||
      !roles.slice(0,3).every((r,i) => r.address === args[i]))
    throw Error('Формат архива Bancor V3 не распознан.');
  const bindings = roles.map(r => [r.address,r.keccak256,r.implementation.address,r.implementation.keccak256].join(' '));
  const configured = new Set((settings.roles ?? []).map(row => row.trim().toLowerCase().split(/\s+/).join(' ')));
  if (settings.kind !== 'bancor_v3' || String(settings.chainId) !== '1' ||
      ![settings.market,settings.quoter,settings.vault,settings.wrapped,settings.native].every((v,i) => same(v,args[i])) ||
      !settings.hashes?.some(h => same(h,plan.runtimeKeccak)) || configured.size !== bindings.length ||
      !bindings.every(row => configured.has(row)))
    throw Error('Настройки Bancor V3 отличаются от проверенных адресов и кода Ethereum.');
  if (await hashCode(plan.data) !== plan.creationSha256 || !plan.data.toLowerCase().endsWith(args.map(word).join('')))
    throw Error('Байткод или аргументы конструктора Bancor V3 изменились.');
  return plan;
}

export async function checkBancor(provider, plan, at) {
  if (at?.requireCanonical !== true || !digest.test(at.blockHash)) throw Error('Требуется канонический блок Ethereum.');
  const rpc = (method, params) => provider.request({method,params});
  const code = addr => rpc('eth_getCode',[addr,at]);
  const [, , , wrapped, native] = plan.arguments;
  const [wrappedCode, nativeCode] = await Promise.all([code(wrapped), code(native)]);
  if (runtimeKeccak(wrappedCode) !== plan.wrappedHash || nativeCode !== '0x')
    throw Error('Код WETH или адрес нативного ETH изменился.');
  await Promise.all(names.map(async name => {
    const role = plan.roles[name];
    const impl = role.implementation;
    const [proxyCode, active, implCode] = await Promise.all([
      code(role.address), rpc('eth_getStorageAt',[role.address,slot,at]), code(impl.address),
    ]);
    if (runtimeKeccak(proxyCode) !== role.keccak256 || !same(active,'0x'+word(impl.address)) ||
        runtimeKeccak(implCode) !== impl.keccak256)
      throw Error('Изменилась реализация Bancor V3 для ' + name + '.');
  }));
}
