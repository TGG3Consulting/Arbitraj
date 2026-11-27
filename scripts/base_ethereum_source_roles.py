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
# Операция конкретного адаптера оборота. Статус относится к паре взаимозаменяемых
# активов в этой записи, а не к другим продуктам того же бренда.
NONSPOT_MEASURES = {
    "dexs/metamask-card.ts": (79, "Сумма токенов из аргументов withdraw в транзакциях по адресам расчёта карты."),
    "dexs/x402/index.ts": (36, "Сумма переводов токенов в платёжных транзакциях авторизованных фасилитаторов x402."),
    "dexs/predictex/index.ts": (79, "Trade и Liquidate измеряют открытие, изменение и ликвидацию позиции на исход события в USDC."),
    "dexs/sablier-lockup.ts": (116, "Получение получателем токенов из Lockup через Withdraw."),
    "dexs/spritz-card.ts": (61, "Сумма paymentTokenAmount события Payment в адрес платёжного обработчика Spritz."),
    "dexs/sablier-flow.ts": (111, "Получение получателем токенов из Flow через Withdraw."),
    "dexs/sablier-legacy.ts": (96, "Вывод получателем токенов из ранее созданного потока."),
    "dexs/llamapay.ts": (75, "Выплата зарплаты получателю из потока LlamaPay."),
    "dexs/superfluid.ts": (250, "Получение токена через CFA и разовые распределения GDA."),
    "dexs/manifold.ts": (51, "Оплата NFT при покупке, принятии предложения или расчёте по аукциону."),
    "fees/collector-crypt/index.ts": (274, "Исходник считает покупки наборов физических коллекционных карт."),
    "fees/central-city.ts": (65, "События FeeCharged за услуги агентов x402. В оборот входит gross по включённому типу платежа. Услуга не образует вторую валюту обмена."),
    "dexs/topmarket/index.ts": (271, "Top.Market суммирует USDC за открытие наборов карт, в том числе позже возвращённые платежи. Сумма покупок пакетов не задаёт исполнимый курс токена."),
    "dexs/thales/index.ts": (320, "Учитываются ставки на спортивные билеты, Speed Markets и цифровые опционы."),
    "fees/dyli/index.ts": (401, "DYLI объединяет оплату физических коллекционных карт, наборов, услуг и операции маркетплейса, включая Stripe. Эта метрика не определяет котировку пары ERC20."),
    "dexs/coinlotto/index.ts": (111, "CoinLotto считает цену лотерейного билета, умноженную на количество. Покупатель получает участие в розыгрыше."),
    "fees/azuro/index.ts": (90, "Azuro суммирует поставленные средства по рассчитанным ставкам трёх версий. Требуемый спотовой паре встречный актив в этом измерении отсутствует."),
    "dexs/sudoswap-v2/index.ts": (16, "Sudoswap V2 использует сделки NFT. Предмет определяется контрактом коллекции и tokenId; объём NFT не является глубиной взаимозаменяемого токена."),
    "dexs/exa-card.ts": (9, "Оплата покупки со счёта карты или в рассрочку. [Условия оплаты Exa](https://help.exactly.app/en/articles/10022626-how-does-the-exa-card-work)."),
    "dexs/b402/index.ts": (38, "Событие Shield учитывает внесённые активы в приватный пул."),
    "dexs/axiumpass.ts": (179, "SubscriptionProcessed учитывает периодические платежи получателям подписки."),
    "dexs/defa/index.ts": (46, "Safe кредитора перечисляет USDC или USDT заёмщику."),
    "dexs/smardex-usdn/index.ts": (30, "Учитывается залог wstETH при открытии и закрытии длинной позиции. Для AMM выделена строка 2695."),
    "dexs/fibo.ts": (26, "Ставки USDC на исход двухминутных раундов."),
    "dexs/hot-take/index.ts": (60, "Контракт блокирует ставки обеих сторон в USDC до расчёта по исходу."),
    "dexs/shiny/index.ts": (201, "Игрок платит USDC за наборы коллекционных карт и участие в игре."),
    "fees/pumpbox/index.ts": (94, "Оплата открытия набора по фиксированной цене USDC."),
    "dexs/grail/index.ts": (21, "Оплата наборов карт в USDC."),
    "dexs/raflux/index.ts": (144, "Оплата наборов физических коллекционных карт в USDC."),
    "dexs/lienfi/index.ts": (26, "Оплачиваются NFT на недвижимость."),
    "fees/flooor-fun/index.ts": (61, "Оборот продаж NFT по выигравшей ставке."),
    "dexs/pixiechess/index.ts": (7, "События Purchased и Mint учитывают покупки игровых наборов и выпуски конкретных фигур."),
    "dexs/sweep-n-flip/index.ts": (145, "Учитываются сделки пулов NFT."),
    "dexs/blur.ts": (5, "Цена исполненных поручений по предметам коллекций, включая Execution721Packed."),
    "dexs/rarible/index.ts": (24, "Оплата заказов NFT. [Декодирование предмета ERC721/ERC1155](https://github.com/DefiLlama/dimension-adapters/blob/be9967c3f0d3ccd06c871bab0d16beb3304de719/dexs/rarible/helper.ts#L37)."),
    "dexs/clutch-anvil/index.ts": (128, "NFTBought и NFTSold связывают платёж с предметом коллекции."),
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


def nonspot(network, adapter):
    """Возвращает операцию с первичным источником только для проверенной строки объёма."""
    card = card_reference(network, adapter)
    if card:
        helper, _ = CARD_PROGRAMS[adapter["module"]]
        method = ("Событие Liquidation учитывает расчёт с карточной программой Rain." if helper == "rainCard"
                  else "Источник суммирует поступления на адреса карточной программы.")
        if network == "ethereum" and adapter["module"] == "cypher-card":
            method += " Спотовые реестры Cypher V2 и V4 учтены в строках 7037 и 7038."
        return f"нет спота · {method} [Исходник учёта]({card})."
    evidence = NONSPOT_MEASURES.get(adapter.get("path"))
    if evidence:
        line, reason = evidence
        ref = f"{adapter['link']}#L{line}"
        return f"нет спота · {reason} [Исходник учёта]({ref})."
    return None


def describe(network,source_id,adapter,configured,candidates):
    path = adapter.get("path")
    revision="be9967c3f0d3ccd06c871bab0d16beb3304de719"
    repository=f"https://github.com/DefiLlama/dimension-adapters/blob/{revision}/"
    # Each entry explains what the cited adapter actually counts on this chain.
    # A terminal's or card programme's turnover is not a second claim on its suppliers' reserves.
    dimensions = {
        "dexs/securedfinance/index.ts": (76, "Срочная книга сопоставляет заёмщика и кредитора. Позицию можно продать до погашения или токенизировать в ERC20. Адрес рынка для отдельного обмена ZC требуется подтвердить. [Правила книги](https://docs.secured.finance/fixed-rate-lending/overview)."),
        "dexs/nftx-v3.ts": (114, "Спот vToken/WETH в AMM NFTX. Источник учитывает также операции с NFT, проведённые через эти пулы. [Роли AMM](https://docs.nftx.io/core-contracts/nftxfeedistributorv3)."),
        "dexs/infinityPools/index.ts": (83, "Оборот спотового обмена берётся по событиям Spot Swap собственных пулов."),
        "fees/lyra-market.ts": (103, "LyraSwapRouter проводит спотовые обмены через Uniswap V2/V3/V4. Одни и те же пулы обслуживают обе строки объёма."),
        "dexs/o1-exchange/index.ts": (150, "Оборот терминала o1 вычислен по комиссии, умноженной на 100. Запись не задаёт адрес пула, резерв или firm quote. Цену может подтвердить только фактический источник обмена."),
        "dexs/bloom.ts": (73, "Bloom сопоставляет сделки нижележащих DEX с адресами роутеров терминала. Один и тот же обмен нельзя прибавить как отдельный остаток Bloom."),
        "dexs/axiom.ts": (125, "Axiom считает сделки нижележащих площадок через контракты терминала. В этой строке нет отдельной кассы Axiom."),
        "dexs/aveai/index.ts": (162, "Ave.ai измеряет обмены через нижележащие DEX, включая исполненные лимитные заявки. Для глубины учитывается источник фактической оплаты каждой ноги."),
        "dexs/p2pme.ts": (1, "выплата USDC покупателю после исполнения заявки на покупку за фиат. Для этой сделки требуется подтверждение фиатной ноги."),
        "dexs/zkp2p.ts": (1, "сумма USDC по исполненным платёжным намерениям в индексе Peer. Для этой сделки требуется подтверждение фиатной ноги."),
    }
    if path in dimensions:
        line, reason=dimensions[path]
        return f"{reason} [Методика]({repository}{path}#L{line})."
    basin={
        ("base","5458","dexs/pinto/index.ts"):("pinto-exchange-base","pinto_base"),
        ("ethereum","4703","dexs/basin/index.ts"):("basin-wells-ethereum","basin_ethereum"),
    }.get((network,source_id,path))
    if network == "ethereum" and source_id == "1700" and "wombat-exchange" not in configured:
        return "оборот < 5 000 USD, отложено"
    if basin:
        profile,stem=basin
        if profile not in configured:
            return "оборот < 5 000 USD, отложено"
        folder=Path("tests/data/first_run_dex_2026-11-26")
        registry=json.loads(gzip.decompress((folder/f"{stem}_wells.json.gz").read_bytes()))
        payments=json.loads(gzip.decompress((folder/f"{stem}_payments.json.gz").read_bytes()))
        settings=configured[profile]["settings"]
        wells=set(registry["wells"])
        if set(settings["dex.stable_pools"])!=wells or any(
                row["roles"]["aquifer"]["address"]!=settings["dex.factory"] for row in registry["wells"].values()):
            raise ValueError(f"{profile} disagrees with its original Aquifer")
        settled=[r for r in payments["cases"] if r.get("v5",{}).get("status")=="paid"]
        return (f"Профиль {profile}, Aquifer {settings['dex.factory']}. На H={registry['block']} сверены "
                f"{len(wells)} скважин, в {len({r['well'] for r in settled})} из них исходный контракт и V5 вернули "
                f"одинаковые денежные дельты в {len(settled)} случаях. "
                f"[Оплата, выходной минимум и срок](../{folder.as_posix()}/{stem}_payments.json.gz).")
    if network == "ethereum":
        observed = {
            "dexs/axiom.ts": "Терминал Axiom. Запрос Dune отбирает сделки DEX по плате Axiom и берёт наибольший шаг каждой транзакции. Физический пул принадлежит площадке исполнения.",
            "dexs/aveai/index.ts": "Маршрутизатор Ave.ai. В объём входят обмены через исходные DEX и исполненные лимитные заявки. Тот же обмен может учитываться в строке его пула.",
            "fees/fwa-v2/index.ts": "Покупка и расчёт по NFT-листингам FWA. Комиссии и выкуп FWA записаны отдельно. Для валютного маршрута нужен конкретный пул выкупа.",
            "dexs/barterswap/index.ts": "Superposition распределяет вход и собирает выход с кошельков поставщиков ликвидности. Для заявки своего плательщика нужен подписанный ответ сервера с выбранными кошельками и сроком. [Архитектура Barter](https://barterdefi.gitbook.io/barter-docs/superposition/architecture-v1).",
            "dexs/dinari/index.ts": "OrderProcessor учитывает денежный объём заявок на цифровые ценные бумаги. Версия 1 читает OrderFill, версия 2 сопоставляет заявки и расчёты. Для новой сделки нужен доступный выпуск, принятая заявка и встречная выплата.",
            "dexs/pancakeswap-x/index.ts": "Аукцион PancakeSwap X. Профиль, собственный плательщик и проверенные выплаты обеих сетей приведены в разделе PancakeSwap X этого справочника.",
        }
        if path in observed:
            return observed[path]
        if path == "dexs/motoswap/index.ts":
            folder = Path(__file__).resolve().parents[1] / "tests/data/first_run_dex_2026-11-26"
            original = json.loads(gzip.decompress((folder / "motoswap_ethereum.json.gz").read_bytes()))
            protected = json.loads(gzip.decompress((folder / "motoswap_ethereum_wallets.json.gz").read_bytes()))
            directory = original["directory"]
            settings = configured["motoswap-ethereum"]["settings"]
            if (original["selector"] != protected["selector"]
                    or directory["roles"]["factory"] != settings["dex.factory"]
                    or directory["roles"]["fee_router"] != settings["dex.quoter"]
                    or protected["manifest"]["runtime"] not in settings["dex.module_runtime_hashes"]):
                raise ValueError("Motoswap: profile differs from paid roles")
            paths = json.loads(gzip.decompress((folder / "motoswap_ethereum_paths.json.gz").read_bytes()))
            if paths["selector"] != original["selector"] or paths["runtime"] not in settings["dex.module_runtime_hashes"]:
                raise ValueError("Motoswap: multi-pool settlement differs from profile")
            usdt = "0xdac17f958d2ee523a2206206994597c13d831ec7"
            weth = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
            case = {p["mode"]:p for p in paths["payments"] if p["path"][0]==usdt and p["path"][-1]==weth and p["spent"]=="1000000"}
            paid, separate = case["guarded"], case["v5_steps"]
            return (f"Профиль `motoswap-ethereum`. Фабрика подтвердила {len(directory['pairs'])} пары. "
                    f"На высоте {original['block']} исходный FeeRouter выплатил {sum(r['paid']['ok'] for r in original['trials'])} обменов. "
                    f"Защищённый модуль и V5 выплатили ещё {len(protected['payments'])}. Для них владелец размещает MotoswapWallet. "
                    f"На этом блоке владелец FeeRouter ещё мог обновлять реализацию. "
                    f"При разрешённом промежуточном MOTO (`dex.intermediate_tokens`) один вызов из 1 USDT выплатил "
                    f"{paid['received']} wei WETH при {paid['gas']} gas. Две отдельные ноги V5 дали {separate['received']} wei при {separate['gas']} gas. "
                    "[Расчёт через две пары](../tests/data/first_run_dex_2026-11-26/motoswap_ethereum_paths.json.gz) · "
                    "[Квитанции FeeRouter](../tests/data/first_run_dex_2026-11-26/motoswap_ethereum.json.gz) · "
                    "[Выплаты, газ и откаты защищённых путей](../tests/data/first_run_dex_2026-11-26/motoswap_ethereum_wallets.json.gz).")
    if network=="base" and source_id=="3409" and path=="dexs/swapbased-v3.ts":
        factory=configured["swapbased-v3"]["settings"]["dex.factory"].lower()
        assert factory=="0xb5620f90e803c7f957a9ef351b8db3c746021bea"
        return f"Профиль `swapbased-v3`. Источник отбирает PoolCreated исходной фабрики `{factory}` и Swap созданных ею пулов. [Код]({repository}{path}#L10)."
    if network=="ethereum" and source_id=="8449" and path=="dexs/wise-token.ts":
        archive=Path("tests/data/first_run_dex_2026-11-26/wise_token_ethereum.json")
        proof=json.loads(archive.read_bytes())
        assert proof["factory"]==configured["uniswap_v2"]["settings"]["dex.factory"].lower()
        assert proof["getPair"]==proof["pool"]=="0x21b8065d10f73ee2e260e5b47d3344d3ced7596e" and all(proof["reserves"][:2])
        return f"Пара WISE/WETH исходного Uniswap V2. На блоке {proof['block']} адрес пары совпал у источника и getPair фабрики профиля `uniswap_v2`. [Ответы пула и фабрики](../{archive.as_posix()})."
    if network=="ethereum" and (source_id,path) in {("7508","dexs/aptos-caliber-prop-amm/index.ts"),("9437","dexs/taurusfi/index.ts")}:
        name="kaliber" if source_id=="7508" else "taurus"
        profile=name+"-lambda-ethereum"
        archive=Path("tests/data/first_run_dex_2026-11-26")/(name+"_ethereum_wallets.json.gz")
        proof=json.loads(gzip.decompress(archive.read_bytes()))
        paid=proof["historicalSettlement"]
        assert paid["roles"]["venue"]["address"]==configured[profile]["settings"]["dex.factory"]
        strict=[r for r in proof["rows"] if r["strict"]]
        assert strict and all(r["status"]=="reverted" for r in strict)
        within_block=archive.with_name(name+"_ethereum_price_slot.json.gz")
        slots=json.loads(gzip.decompress(within_block.read_bytes()))["rows"]
        protected=[r for r in slots if r["strict"]]
        own=[r for r in protected if r["status"]=="paid" and r["actual"]==paid["roles"]["venue"]["address"]]
        assert all(r["withoutUpdate"]["status"]=="reverted" for r in protected)
        assert len(own)==(4 if name=="kaliber" else 9)
        feeds=archive.with_name(name+"_ethereum_builder_wallets.json.gz")
        builders=json.loads(gzip.decompress(feeds.read_bytes()))["builders"]
        outcomes=[]
        for feed in builders:
            tested=[r for r in feed["rows"] if r["form"]=="strict"]
            named=[r for r in tested if r["withFrame"].get("actual")==paid["roles"]["venue"]["address"] and r["withFrame"]["status"]=="paid"]
            outcomes.append(f"{feed['builder']} {len(named)}/{len(tested)}")
        counts=", ".join(outcomes)
        return (f"Профиль `{profile}`, фактическая выплата {paid['received']/1_000_000:.6f} USDC в блоке {paid['height']}. "
                f"При проверке после блока все {len(strict)} требований выплаты исходным DEX отклонены. "
                f"Разрешённая замена рассчиталась через Uniswap V3. На том же родителе, после подписанного обновления цены "
                f"внутри целевого блока исходная площадка оплатила {len(own)} из {len(protected)} проверок. Без обновления — 0. "
                f"Публичный кадр строителя, H26507885: {counts} направлений дали выплату на проверенном объёме. "
                f"[Квитанция и проверки](../{archive.as_posix()}) · [Сравнение в слоте](../{within_block.as_posix()}) · [Кадр и выплаты](../{feeds.as_posix()}).")
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
