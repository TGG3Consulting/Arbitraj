// Область ручного запроса Hashflow. Адрес trader и сеть возвращает сервер после симуляции.
export function sameHashflowWallet(original, venue, trader, chain, details) {
  return original.venue===venue && original.chain===chain && (chain==null || details.chain_id===chain) &&
    original.trader===trader?.toLowerCase() && details.trader?.toLowerCase()===original.trader;
}
