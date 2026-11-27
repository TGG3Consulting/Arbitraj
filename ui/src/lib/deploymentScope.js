// Одна операция кошелька привязана к профилю, открытому до первого асинхронного запроса.
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
    const target = kind === 'registered_uni_v3' ? 'dex.v3_wallet_module' : 'dex.router';
    return setValue(target, {kind:'Dex', id}, address);
  }
  return {key, settings:JSON.parse(settingsJson), active, check, read, save, beforeOrAfterSend, install};
}
