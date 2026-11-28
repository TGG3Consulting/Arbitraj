const isAddress = value => typeof value === 'string' && /^0x[0-9a-fA-F]{40}$/.test(value);
const positiveRaw = value => typeof value === 'string' && /^[1-9][0-9]{0,77}$/.test(value) && BigInt(value) < 2n ** 256n;
export const tether = '0xdac17f958d2ee523a2206206994597c13d831ec7';

export function previewRequest({token, buy, size, firm, minimum, maximum, wallet, trader}) {
  if (!isAddress(token) || !isAddress(trader) || !wallet || token.toLowerCase() === tether || !positiveRaw(size)) return null;
  if (firm && (!positiveRaw(minimum) || (buy && !positiveRaw(maximum)))) return null;
  return {
    input: buy ? tether : token,
    output: buy ? token : tether,
    trader,
    ...(buy ? {buy_amount_raw:size} : {amount_raw:size}),
    ...(firm ? {firm:{wallet, minimum_raw:minimum, ...(buy ? {maximum_raw:maximum} : {}), allow_trade:false}} : {})
  };
}
