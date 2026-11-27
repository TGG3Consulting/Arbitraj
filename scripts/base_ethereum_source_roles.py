"""Attribute a source row to a spot deployment only for the documented protocol and chain role."""

import gzip
import json
from pathlib import Path

# Explicit roles prevent a shared Uniswap V4 PoolManager, ERC20 or permit2 address from identifying a DEX.
MARKETS = {
    "base": {
        "799": ("dex.factory", ["rubicon-base","rubicon-clmm-base","rubicon-classic-base"]),
        "2197": ("dex.factory", ["uniswap-v2-base"]),
        "6133": ("dex.pool_manager", ["pancakeswap-infinity-clmm-base","pancakeswap-infinity-lbamm-base"]),
        "3255": ("dex.factory", ["horizondex-base"]),
        "8582": ("dex.factory", ["umbrae-damm","umbrae-dlmm"]),
        "1883": ("dex.factory", ["iziswap-base"]),
        "6620": ("dex.factory", ["thirdfy"]),
        "146": ("dex.factory", ["dodo-amm-dvm","dodo-amm-dpp","dodo-amm-dsp"]),
    },
    "ethereum": {
        "799": ("dex.factory", ["rubicon-aquila-ethereum","rubicon-clmm-ethereum"]),
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
    "dexs/metamask-card.ts": "сумма токенов из аргументов withdraw в транзакциях по адресам расчёта карты",
    "dexs/x402/index.ts": "сумма переводов токенов в платёжных транзакциях авторизованных фасилитаторов x402",
    "dexs/predictex/index.ts": "оборот позиции по бессрочному контракту, рассчитанный по событиям Trade и Liquidate",
    "dexs/sablier-lockup.ts": "получение получателем токенов из Lockup через Withdraw",
    "dexs/spritz-card.ts": "сумма paymentTokenAmount события Payment в адрес платёжного обработчика Spritz",
    "dexs/p2pme.ts": "выплата USDC покупателю после исполнения заявки на покупку за фиат",
    "dexs/zkp2p.ts": "сумма USDC по исполненным платёжным намерениям в индексе Peer",
    "dexs/sablier-flow.ts": "получение получателем токенов из Flow через Withdraw",
    "dexs/sablier-legacy.ts": "вывод получателем токенов из ранее созданного потока",
    "dexs/llamapay.ts": "выплата зарплаты получателю из потока LlamaPay",
    "dexs/superfluid.ts": "получение токена через CFA и разовые распределения GDA",
    "dexs/manifold.ts": "оплата NFT при покупке, принятии предложения или расчёте по аукциону",
}

# The dimension adapters store these addresses outside their network stanzas.
# Each value identifies the *role* used by that adapter; never match arbitrary token addresses.
SOURCE_ROLES = {
    "2611": ("dex.vault","0xba12222222228d8ba445958a75a0704d566bf2c8","dexs/balancer-v2.ts",14,
             {"base":"balancer-v2-base","ethereum":"balancer_ethereum"}),
    "5491": ("dex.vault","0xba1333333333a1ba1108e8412f11850a5c319ba9","dexs/balancer-v3/index.ts",23,
             {"base":"balancer-v3-base","ethereum":"balancer-v3-ethereum"}),
    "1995": ("dex.factory","0xeef417e1d5cc832e619ae18d2f140de2999dd4fb","dexs/bancor-v3.ts",10,
             {"ethereum":"bancor-v3-ethereum"}),
}

# These exported modules share a helper; their individual catalog entries contain no path.
CARD_PROGRAMS = {
    "cypher-card": ("crypto-card",127), "bleap-card": ("crypto-card",153),
    "nexo-card": ("crypto-card",177), "holyheld": ("crypto-card",207),
    "redotpay": ("crypto-card",245), "karta-card": ("rainCard",117),
    "tuyo-card": ("rainCard",130), "kolo-card": ("rainCard",143),
}

def card_reference(network, adapter):
    name=adapter.get("module")
    if name not in CARD_PROGRAMS or (network != "base" and name in {"karta-card","tuyo-card"}):
        return None
    helper,line=CARD_PROGRAMS[name]
    return f"https://github.com/DefiLlama/dimension-adapters/blob/be9967c3f0d3ccd06c871bab0d16beb3304de719/helpers/{helper}.ts#L{line}"


def describe(network,source_id,adapter,configured,candidates):
    path = adapter.get("path")
    revision="be9967c3f0d3ccd06c871bab0d16beb3304de719"
    repository=f"https://github.com/DefiLlama/dimension-adapters/blob/{revision}/"
    card = card_reference(network,adapter)
    if card:
        helper,_ = CARD_PROGRAMS[adapter["module"]]
        if helper=="rainCard":
            return "Расход по карте. Источник суммирует выплаченные токены из Liquidation связанного с картой залогового контракта Rain."
        return "Расход по карте. Источник суммирует токены, поступившие на указанные получающие адреса карточной программы."
    if network=="ethereum" and source_id=="2121" and path=="dexs/frax-swap/index.ts":
        factory=configured["fraxswap_ethereum"]["settings"]["dex.factory"]
        return (f"Fraxswap. Профиль `fraxswap_ethereum`, реестр `{factory}`, проверено 30 пар. "
                "Источник оборота возвращает общую сумму сети за дату без идентификаторов пулов.")
    if network=="ethereum" and source_id=="4944" and path=="dexs/bob-gateway/index.ts":
        return ("Межсетевой обмен BTC ↔ USDT. Одна нога требует перевода Bitcoin и его подтверждения. "
                "На Ethereum учитывается выпуск USDT или завершённая заявка после доказательства оплаты BTC.")
    role=SOURCE_ROLES.get(source_id)
    if network=="ethereum" and source_id=="7381" and path=="dexs/supernova.ts":
        role=("dex.factory","0x5aef44edfc5a7edd30826c724ea12d7be15bdc30",path,8,{"ethereum":"supernova-amm"})
    if source_id=="2695" and path=="dexs/smardex/index.ts":
        factory,line={"base":("0xdd4536dd9636564d891c919416880a3e250f975a",6),
                      "ethereum":("0xb878dc600550367e14220d4916ff678fb284214f",8)}[network]
        role=("dex.factory",factory,"dexs/smardex/config.ts",line,{network:"smardex-"+network})
    if role:
        key,address,file,line,markets=role
        if network not in markets:
            return None
        id=markets[network]
        if configured[id]["settings"][key].lower()!=address:
            raise ValueError(f"{network}/{id}: configured role differs from {file}:{line}")
        return (f"Профиль `{id}` использует `{key} = {address}`. "
                f"[Роль контракта в источнике]({repository}{file}#L{line}).")
    if path=="dexs/native/index.ts" and source_id=="2803":
        fixture={"base":"native_rfq_base_roles.json","ethereum":"native_rfq_ethereum_protected.json.gz"}[network]
        return (f"Native RFQ · источник ищет RFQTrade по сигнатуре без ограничения адреса. "
                f"Проверенные Router и RFQ pool для этой сети указаны в `native_rfq.roles`; "
                f"для конкретной сделки нужен действующий firm quote на собственный кошелёк. "
                f"[Контракты и архив сети](../tests/data/first_run_dex_2026-11-26/{fixture}).")
    if path=="dexs/gmgnai.ts":
        return "Терминал GMGN считает сделки на сторонних DEX с оплатой получателю GMGN. Доход пользователя зависит от пулов исполнения каждой операции."
    if path=="dexs/avici.ts":
        return "Источник получает долларовую сумму обменов через Relay из API Avici. Отдельный контракт ликвидности из агрегата не определяется."
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
    if source_id=="799":
        data = Path(__file__).resolve().parents[1]/"tests/data/first_run_dex_2026-11-26"
        roles = json.loads((data/"gladius_source_roles.json").read_bytes())[network]
        archive = f"gladius_{network}_protected.json.gz"
        proof = json.loads(gzip.decompress((data/archive).read_bytes()))
        records = roles["records"]
        reactor = proof["roles"]["reactor"]["address"]
        if roles["selector"] != proof["selector"] or {r["emitter"] for r in records} != {reactor}:
            raise ValueError("Gladius: receipt emitter or settlement block differs")
        second = roles.get("source_second_address")
        expected = {reactor} | ({second["address"]} if second else set())
        if addresses-matched != expected:
            raise ValueError(f"Rubicon: unresolved {network} contract role")
        text += f" Gladius: {len(records)} исторических оплат, Fill выпущен `{reactor}`."
        if second:
            paid = sum(r["filler"] == second["address"] for r in records)
            text += (f" Второй адрес источника `{second['address']}` выступил плательщиком в {paid} квитанциях, "
                     "его reactor() вернул первый адрес.")
        text += (f" На собственных подписанных тестовых ордерах защищённый модуль провёл {len(proof['cases'])} ERC20-обмена. "
                 "В настройках Gladius нужны адрес и код своего модуля; для новой цены нужен действующий подписанный ордер, "
                 "актуальный расход по Quoter и оплачиваемая симуляция со стороны владельца.")
        observations = roles["current_public_get"]
        if "error" in observations:
            text += f" Замер официального GET в {observations['at']}: {observations['error']}."
        elif observations.get("status") != 200:
            text += f" Замер официального GET в {observations['at']}: HTTP {observations['status']}."
        text += (f" [Кассовые квитанции](../tests/data/first_run_dex_2026-11-26/{archive}) · "
                 "[Размещения, роли в Fill и хеши квитанций](../tests/data/first_run_dex_2026-11-26/gladius_source_roles.json).")
        return text
    if addresses-matched:
        text += f" Других адресов в сетевой секции: {len(addresses-matched)}. Их назначение проверяется отдельно."
    return text
