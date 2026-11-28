// Одна операция кошелька привязана к профилю, открытому до первого асинхронного запроса.
/** В сети у общего модуля один адрес и один журнал CREATE для всех кошельков и версий карточки. */
export function sourceDeploymentKey(source,venue,chain,version,wallet,shared) {
  return (shared ? [source+'-create',venue,chain,'shared'] :
    [source+'-create',venue,chain,version,wallet.toLowerCase()]).join(':');
}

/** Карточки идут от новой к старой. Поручение и ручной CREATE ограничивают выбор точным кодом. */
export function matchingCard(settings, pending, input, cards) {
  const allowed = new Set((settings.hashes ?? []).map(h => h.toLowerCase()));
  return cards.find(c => allowed.has(c.runtimeKeccak) &&
    (!pending || c.creationSha256 === pending.creation) &&
    (input === undefined || c.data.toLowerCase() === input?.toLowerCase()));
}

export function deploymentScope(storage, selected) {
  const initial = selected();
  if (!initial?.key) throw Error('Параметры размещения недоступны.');
  const {id, venue, kind, key} = initial;
  const settingsJson = JSON.stringify(initial.settings);
  function active() {
    const next = selected();
    return !!next && next.id === id && next.venue === venue && next.kind === kind && next.key === key
      && JSON.stringify(next.settings) === settingsJson;
  }
  function check() {
    if (!active()) throw Error('Выбранная сеть или настройки DEX изменились. Откройте параметры размещения заново.');
  }
  function available() {
    check();
    if (selected().router) throw Error('Адрес модуля уже настроен.');
  }
  const read = () => JSON.parse(storage.getItem(key) ?? 'null');
  function save(record) {
    if (record) storage.setItem(key, JSON.stringify(record));
    else storage.removeItem(key);
  }
  let reserved = false;
  function beforeOrAfterSend(record) {
    if (!reserved) {
      available();
      if (read()) throw Error('Есть ранее начатое размещение.');
      save(record);
      reserved = true;
    } else {
      // Хеш или отказ кошелька относится к первоначальному CREATE даже после смены экрана.
      save(record);
    }
  }
  function install(address, setValue) {
    available();
    const target = ['registered_uni_v3','algebra_integral_theorem'].includes(kind) ? 'dex.v3_wallet_module' : 'dex.router';
    return setValue(target, {kind:'Dex', id}, address, initial.router ?? null);
  }
  const settings=JSON.parse(settingsJson);
  /** Значение источника сохраняется в Venue с условием на исходное значение, включая отсутствие адреса. */
  async function installSource(setting,next,setValue) {
    check();
    if (next!==settings.rows) await setValue(setting,{kind:'Venue',id:venue},next,settings.rows);
  }
  return {key, settings, active, check, read, save, beforeOrAfterSend, install, installSource};
}
