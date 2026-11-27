// Две поставляемые карточки Dexalot. Существующий CREATE восстанавливает walletDeployment.deployed,
// а новую отправку SourceWalletModule всегда готовит по текущей карточке.
import {loadPlan, sameRuntime} from './walletDeployment.js';
import {findWalletModule} from './registeredWalletModule.js';

const legacy = async () => (await import('../assets/dexalot-base-create-legacy.json')).default;

/** Возвращает проверенную карточку исходного поручения или введённого вручную CREATE. */
export async function recoverDexalot(provider,settings,current,pending,hash,previous=legacy) {
  if (pending && (pending.from!==current.wallet || pending.chain!==current.chainId)) {
    throw Error('Исходное поручение принадлежит другому кошельку или сети.');
  }
  const tx=pending ? null : await provider.request({method:'eth_getTransactionByHash',params:[hash]});
  const matches=pending ? card=>card.creationSha256===pending.creation :
    card=>tx?.input?.toLowerCase()===card.data.toLowerCase();
  if (matches(current)) return current;
  const old=await previous();
  if (!matches(old)) throw Error('Для исходного CREATE нет проверенной карточки Dexalot.');
  return loadPlan(settings,old);
}

/** Прежняя версия в реестре владельца также исключает повторное платное размещение. */
export async function registeredDexalotVersions(provider,current,rows,previous=legacy) {
  const modules=rows.filter(row=>typeof row==='string')
    .map(row=>row.trim().toLowerCase().split(/\s+/))
    .filter(r=>r.length===3 && r[0]==='module' && /^0x[0-9a-f]{40}$/.test(r[1]));
  const addresses=card=>modules.filter(r=>r[2]===card.runtimeKeccak).map(r=>r[1]);
  const found=await findWalletModule(provider,current,addresses(current),sameRuntime);
  if (found || !modules.length) return found;
  const old=await previous();
  const candidates=addresses(old);
  if (!candidates.length) return null;
  const plan=await loadPlan({kind:current.kind,chainId:current.chainId,wallet:current.wallet,rows},old);
  return findWalletModule(provider,plan,candidates,sameRuntime);
}
