const isAddress = value => typeof value === 'string' && /^0x[0-9a-fA-F]{40}$/.test(value);
const positiveRaw = value => typeof value === 'string' && /^[1-9][0-9]{0,77}$/.test(value) && BigInt(value) < 2n ** 256n;

export function previewRequest({token, cash, buy, size, firm, minimum, maximum, wallet, trader}) {
  if (!isAddress(token) || !isAddress(cash) || !isAddress(trader) || !wallet || token.toLowerCase() === cash.toLowerCase() || !positiveRaw(size)) return null;
  if (firm && (!positiveRaw(minimum) || (buy && !positiveRaw(maximum)))) return null;
  return {
    input: buy ? cash : token,
    output: buy ? token : cash,
    trader,
    ...(buy ? {buy_amount_raw:size} : {amount_raw:size}),
    ...(firm ? {firm:{wallet, minimum_raw:minimum, ...(buy ? {maximum_raw:maximum} : {}), allow_trade:false}} : {})
  };
}
