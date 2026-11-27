// Неизменяемые адреса оригинальной фабрики V1 и канонического WETH, принятые архивом на Ethereum.
import sha3 from 'js-sha3';

export function runtimeKeccak(code) {
  if (typeof code !== 'string' || !/^0x([0-9a-f]{2})+$/i.test(code)) throw Error('Код контракта отсутствует.');
  return '0x'+sha3.keccak_256(Uint8Array.from(code.slice(2).match(/../g), (s) => parseInt(s,16)));
}

const address = /^0x[0-9a-f]{40}$/;
const word = /^0x[0-9a-f]{64}$/;
const sha = /^[0-9a-f]{64}$/;
const same = (a, b) => typeof a === 'string' && a.toLowerCase() === b;
const argument = (a) => a.slice(2).padStart(64, '0');

export async function loadV1(s, p, hashCode) {
  const roles = p.roles ?? {};
  const {factory, template, wrapped} = roles;
  if (p.kind !== 'uniswap_v1' || p.chainId !== 1 || ![factory,template,wrapped].every((r) =>
    r && address.test(r.address) && !/^0x0+$/.test(r.address) && word.test(r.keccak256) && sha.test(r.sha256)) ||
    new Set([factory.address,template.address,wrapped.address]).size !== 3 || !word.test(p.runtimeKeccak) ||
    !sha.test(p.runtimeSha256) || !word.test(p.cloneHash) || !word.test(p.sourceBlock) ||
    !/^0x[0-9a-f]{8}$/.test(p.templateGetter)) throw Error('Формат архива Uniswap V1 не распознан.');
  if (s.kind !== 'uniswap_v1' || String(s.chainId) !== '1' || !same(s.market,factory.address) ||
    !same(s.factoryHash,factory.keccak256) || !same(s.template,template.address) ||
    !same(s.templateHash,template.keccak256) || !same(s.wrapped,wrapped.address) ||
    !same(s.cloneHash,p.cloneHash) || !s.hashes?.some((h) => same(h,p.runtimeKeccak)))
    throw Error('Сеть, фабрика, шаблон, WETH или допустимый runtime отличаются от настроек этой площадки.');
  if (await hashCode(p.data) !== p.creationSha256 ||
    !p.data.toLowerCase().endsWith(argument(factory.address)+argument(wrapped.address)))
    throw Error('Байткод создания или аргументы конструктора Uniswap V1 изменились.');
  return p;
}

export async function checkV1(provider, p, at, hashCode) {
  if (!word.test(at?.blockHash) || at.requireCanonical !== true) throw Error('Требуется канонический блок Ethereum.');
  const {factory,template,wrapped} = p.roles;
  const rpc = (method, params) => provider.request({method,params});
  await Promise.all([factory,template,wrapped].map(async (r) => {
    const code = await rpc('eth_getCode',[r.address,at]);
    if (await hashCode(code) !== r.sha256 || runtimeKeccak(code) !== r.keccak256) throw Error('Код фабрики, шаблона или WETH изменился.');
  }));
  const value = await rpc('eth_call',[{to:factory.address,data:p.templateGetter},at]);
  if (!same(value,'0x'+argument(template.address))) throw Error('Фабрика V1 вернула другой шаблон биржи.');
}
