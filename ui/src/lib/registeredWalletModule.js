import {runtimeKeccak} from './uniswapV1Deploy.js';
const digest=/^0x[0-9a-f]{64}$/;

export async function findWalletModule(provider,p,candidates){
  if (!candidates.length) return null;
  const chain = await provider.request({method:'eth_chainId',params:[]});
  if (BigInt(chain)!==BigInt(p.chainId)) throw Error('Выберите сеть размещения в кошельке.');
  const head=await provider.request({method:'eth_getBlockByNumber',params:['latest',false]});
  if (!digest.test(head?.hash)) throw Error('Не удалось проверить блок размещения кошелька.');
  const at={blockHash:head.hash,requireCanonical:true};
  for (const target of new Set(candidates)) {
    const code=await provider.request({method:'eth_getCode',params:[target,at]});
    if (code==='0x' || runtimeKeccak(code)!==p.runtimeKeccak) continue;
    const owner=await readWalletOwner(provider,target,at);
    if (owner===p.wallet) return target;
  }
  return null;
}

async function readWalletOwner(provider,target,at) {
  const result=await provider.request({method:'eth_call',params:[{to:target,data:'0x8da5cb5b'},at]});
  if (!/^0x0{24}[0-9a-f]{40}$/i.test(result)) throw Error('Не удалось проверить владельца модуля кошелька.');
  return '0x'+result.slice(-40).toLowerCase();
}

export async function checkWalletOwner(provider,p,target,at) {
  if (await readWalletOwner(provider,target,at)!==p.wallet) throw Error('У этого модуля другой владелец.');
}

