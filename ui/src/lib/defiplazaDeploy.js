// Роли и параметры размещения DefiPlazaWallet, подтверждённые на одном каноническом блоке.
const generations = {defiplaza_multi:false, defiplaza_stable:true};
const digest = /^0x[0-9a-f]{64}$/;
const address = /^0x[0-9a-f]{40}$/;
const same = (a,b) => typeof a === 'string' && a.toLowerCase() === b;
const word = raw => raw.slice(2).padStart(64,'0');

export const defiPlaza = kind => Object.hasOwn(generations,kind);

export async function loadDefiPlaza(settings, plan, sha) {
  if (!defiPlaza(settings.kind) || settings.kind!==plan.kind || String(settings.chainId)!=='1' || plan.chainId!==1 ||
      plan.stable!==generations[plan.kind] || !digest.test(plan.executorHash) || !digest.test(plan.sourceBlock) ||
      !digest.test(plan.runtimeKeccak) || !settings.hashes?.some(h=>same(h,plan.runtimeKeccak))) {
    throw Error('Сеть, поколение или код DefiPlazaWallet расходятся с профилем.');
  }
  for (const [key,configured] of [['factory',settings.market],['wrapped',settings.wrapped]]) {
    const role=plan.roles?.[key];
    if (!role || !address.test(role.address) || /^0x0+$/.test(role.address) || !digest.test(role.keccak256) ||
        !same(configured,role.address)) throw Error('Проверьте адрес кассы и WETH в профиле DefiPlaza.');
  }
  const {factory,wrapped}=plan.roles;
  const args=word(factory.address)+word(wrapped.address)+(plan.stable?'1':'0').padStart(64,'0')+plan.executorHash.slice(2);
  if (factory.address===wrapped.address || !same(settings.factoryHash,factory.keccak256) || !plan.data.endsWith(args) ||
      await sha(plan.data)!==plan.creationSha256 || !/^[0-9a-f]{64}$/.test(plan.runtimeSha256)) {
    throw Error('Код создания или параметры DefiPlazaWallet изменились.');
  }
  return plan;
}
