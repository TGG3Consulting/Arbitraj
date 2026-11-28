// Настройки Base для карточек Hanji и Ryze. Источники — /api/settings/schema и точные значения Venue.
/** Возвращает chain ID выбранной Base. До настройки редактировать можно именованный профиль из схемы. */
export function baseNetwork(venue,profiles,values,allowSetup=false) {
  const id = values['dex.evm_chain_id|Venue|'+venue];
  const chain = values['dex.chain|Venue|'+venue];
  const named = profiles?.find(p=>p.venue===venue);
  const profile = named ?? profiles?.find(p=>p.evm_chain_id===id);
  if (profile?.chain!=='base') return null;
  const setup = allowSetup && named!==undefined;
  return (id===profile.evm_chain_id || (setup && id==null)) &&
    (chain===profile.chain || (setup && chain==null)) ? profile.evm_chain_id : null;
}
