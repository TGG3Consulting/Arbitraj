"""Attribute a source row to a spot deployment only for the documented protocol and chain role."""

# Explicit roles prevent a shared Uniswap V4 PoolManager, ERC20 or permit2 address from identifying a DEX.
MARKETS = {
    "base": {
        "2197": ("dex.factory", ["uniswap-v2-base"]),
        "6133": ("dex.pool_manager", ["pancakeswap-infinity-clmm-base","pancakeswap-infinity-lbamm-base"]),
        "3255": ("dex.factory", ["horizondex-base"]),
        "8582": ("dex.factory", ["umbrae-damm","umbrae-dlmm"]),
        "1883": ("dex.factory", ["iziswap-base"]),
        "6620": ("dex.factory", ["thirdfy"]),
        "146": ("dex.factory", ["dodo-amm-dvm","dodo-amm-dpp","dodo-amm-dsp"]),
    },
    "ethereum": {
        "2776": ("dex.factory", ["sushiswap-v3-ethereum"]),
        "2769": ("dex.factory", ["pancakeswap-v3-ethereum"]),
        "2196": ("dex.factory", ["uniswap-v1-ethereum"]),
        "146": ("dex.factory", ["dodo-pmm-ethereum","dodo-amm-dpp","dodo-amm-dsp"]),
        "8762": ("dex.vault", ["ranges-finance"]),
        "2529": ("dex.factory", ["pancakeswap-stableswap-ethereum"]),
        "2906": ("dex.factory", ["traderjoe-v2-1-ethereum"]),
    }
}
# These particular statistics count a payment to a receiver. No pair, exchange rate or second transfer
# can be inferred from the statistic. A separate strategy or exchange from that provider is independently reviewed.
MEASURE = {
    "dexs/sablier-lockup.ts": "получение получателем токенов из Lockup через Withdraw",
    "dexs/sablier-flow.ts": "получение получателем токенов из Flow через Withdraw",
    "dexs/sablier-legacy.ts": "вывод получателем токенов из ранее созданного потока",
    "dexs/llamapay.ts": "выплата зарплаты получателю из потока LlamaPay",
    "dexs/superfluid.ts": "получение токена через CFA и разовые распределения GDA",
    "dexs/manifold.ts": "оплата NFT при покупке, принятии предложения или расчёте по аукциону",
}

def describe(network,source_id,adapter,configured,candidates):
    path = adapter.get("path")
    if path in MEASURE:
        return "Показатель источника — "+MEASURE[path]+". Эта запись не содержит обмена двух взаимозаменяемых токенов."
    market = MARKETS[network].get(source_id)
    if not market:
        return None
    key,ids = market
    addresses = {str(a).lower() for a in candidates}
    matched = set()
    for id in ids:
        address = configured[id]["settings"][key].lower()
        if address not in addresses:
            raise ValueError(f"{network}/{source_id}/{id}: {key} {address} absent from the network stanza")
        matched.add(address)
    text = "Совпадение по "+key+" в исходнике и профиле "+", ".join(f"`{id}`" for id in ids)+"."
    if addresses-matched:
        text += f" Других адресов в сетевой секции: {len(addresses-matched)}. Их назначение проверяется отдельно."
    return text
