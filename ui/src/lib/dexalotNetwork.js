// Сеть карточки Dexalot и её подтверждённых ролей. Используют экран кошелька,
// форма ключа и проверка карточки. Источник — tests/data/first_run_dex_2026-11-26/dexalot_base_indicative.json.
export const DEXALOT_CHAIN_ID = 8453;
export const DEXALOT_KEY = 'dexalot.api_key';

/** Возвращает ID зарегистрированных DEX с сетью карточки RFQ. */
export function dexalotVenues(venueKinds, values) {
  return venueKinds
    .filter(({id, kind}) => kind === 'Dex' && Number(values['dex.evm_chain_id|Venue|' + id]) === DEXALOT_CHAIN_ID)
    .map(({id}) => id);
}
