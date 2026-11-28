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
# Один проверенный профиль той же площадки и сети достаточен для счётчика строк.
# Остальные поколения и кассы перечислены в адресной части справочника.
COVERAGE_PROFILES = {
    "base": {
        "3": "curve-base", "119": "sushiswap-v2-base", "146": "dodo-amm-dvm", "194": "pancakeswap-v2-base",
        "306": "quickswap-v2-base", "382": "pendle-base", "420": "elk-base", "799": "rubicon-base",
        "1461": "woofi-swap", "1700": "wombat-exchange", "1790": "kokonutswap-base", "1883": "iziswap-base",
        "2197": "uniswap-v2-base", "2198": "uniswap-v3-base", "2611": "balancer-v2-base",
        "2615": "kyberswap-elastic-base", "2644": "maverick-v1", "2695": "smardex-base",
        "2769": "pancakeswap-v3-base", "2776": "sushiswap-v3-base", "2837": "wagmi-base", "2926": "e3-base",
        "3234": "derpdex-base", "3246": "sobal-base", "3255": "horizondex-base", "3314": "rocketswap",
        "3328": "swapbased", "3333": "baseswap", "3345": "dackieswap-v3-base", "3361": "alien-base",
        "3382": "throne-v3-base", "3409": "swapbased-v3", "3450": "aerodrome-base", "3481": "solidly-v3-base",
        "3507": "baseswap-v3", "3515": "dackieswap-v2-base", "3533": "kinetix-v2-base",
        "3575": "equalizer-base", "3714": "sharkswap-base", "3878": "thick-base", "3888": "alien-base-v3",
        "3940": "archly-base", "4062": "deltaswap-base", "4294": "infusion", "4524": "aerodrome-slipstream",
        "4725": "spectra-v2-base", "4752": "maverick-v2-base", "4962": "fwx-base", "5317": "fluid-base",
        "5416": "9mm-v2-base", "5417": "9mm-v3-base", "5423": "hydrometer-base", "5491": "balancer-v3-base",
        "5531": "goblins-base", "5575": "virtuals-base", "5659": "treble-spot", "5690": "uniswap-v4-base",
        "6133": "pancakeswap-infinity-clmm-base", "6365": "hydrex-integral", "6506": "omni-exchange-v3-base",
        "6539": "omni-exchange-v2-base", "6540": "omni-exchange-v4-base", "6576": "quickswap-v4-base",
        "6620": "thirdfy", "7077": "stabull", "7260": "elfomofi", "7304": "sectorone-v2-0-base",
        "7526": "velox-base", "7574": "virtus-amm-base", "7575": "virtus-cl-base", "7587": "treble-v4",
        "7601": "lunarbase", "7882": "topaz-cl", "8209": "1inch-aqua", "8306": "metric-base",
        "8513": "umia-base", "8582": "umbrae-damm", "8621": "basestonk-base", "8691": "aero-slipstream-base",
        "8934": "baibai", "9154": "nami", "9172": "aero-stable-base", "9327": "vectaswap-base",
    },
    "ethereum": {
        "3": "curve", "116": "balancer-v1-ethereum", "119": "sushiswap", "127": "kyberswap_classic_ethereum",
        "146": "dodo-pmm-ethereum", "162": "bancor-v21-ethereum", "194": "pancakeswap_ethereum",
        "202": "saddle-finance", "221": "defi_swap", "291": "integral-size-ethereum", "292": "swapr_ethereum",
        "324": "unicly", "327": "youswap-ethereum", "382": "pendle", "397": "shibaswap",
        "398": "apeswap_ethereum", "404": "bxh-ethereum", "420": "elk_finance_ethereum",
        "595": "convergence-finance-v2", "707": "luaswap-ethereum", "728": "defiplaza-multi-ethereum",
        "799": "rubicon-aquila-ethereum", "812": "empire-ethereum", "863": "yoshi-ethereum",
        "911": "sumswap-ethereum", "1053": "mooniswap", "1616": "radioshack_ethereum", "1732": "verse",
        "1824": "swapsicle-ethereum", "1896": "justmoney-ethereum", "1995": "bancor-v3-ethereum",
        "2121": "fraxswap_ethereum", "2178": "templedao", "2196": "uniswap-v1-ethereum", "2197": "uniswap_v2",
        "2198": "uniswap_v3", "2400": "solidlydex", "2529": "pancakeswap-stableswap-ethereum",
        "2611": "balancer_ethereum", "2615": "kyberswap_elastic", "2644": "maverick-v1",
        "2695": "smardex-ethereum", "2769": "pancakeswap-v3-ethereum", "2776": "sushiswap-v3-ethereum",
        "2782": "aktionariat-ethereum", "2837": "wagmi-ethereum", "2890": "carbon-defi-ethereum",
        "2906": "traderjoe-v2-1-ethereum", "3104": "ambient-ethereum", "3481": "solidly-v3",
        "3499": "ekubo-v2-ethereum", "3940": "archly-ethereum", "4062": "deltaswap-ethereum",
        "4222": "ring-exchange-ethereum", "4321": "curve-llamalend-ethereum", "4752": "maverick-v2-eth",
        "5317": "fluid-ethereum", "5491": "balancer-v3-ethereum", "5690": "uniswap-v4-ethereum",
        "6358": "eulerswap-v1-ethereum", "6483": "angstrom-ethereum", "6586": "fluid-dex-lite-ethereum",
        "6771": "yield-basis-ethereum", "6815": "shibaswap-v2-ethereum", "7037": "cypher-v2-ethereum",
        "7038": "cypher-v4-ethereum", "7077": "stabull-ethereum", "7304": "sectorone-v2-0-ethereum",
        "7381": "supernova-amm", "7382": "supernova-cl", "7501": "wswap-ethereum",
        "7508": "kaliber-lambda-ethereum", "7599": "lista-dex-v3", "7882": "topaz-cl",
        "7984": "kipseli-lambda-ethereum", "8209": "1inch-aqua", "8278": "ammalgam", "8306": "metric-ethereum",
        "8531": "tempest-lambda-ethereum", "8691": "aero-slipstream-ethereum", "8762": "ranges-finance",
        "8858": "motoswap-ethereum", "9154": "nami", "9157": "communityv3", "9435": "goliath-lambda-ethereum",
        "9436": "stelaxis-ethereum", "9437": "taurus-lambda-ethereum", "9514": "theorem-ethereum",
    },
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
    "dexs/landbid.ts": (117, "Объём измеряет ETH, уплаченные за захват территории в игре. Выручка проекта от LP Uniswap входит в другую метрику."),
    "dexs/yfx-v4/index.ts": (35, "Суммируется номинал сделок рынка бессрочных контрактов V4. [Условия рынка YFX](https://docs.yfx.com/yfx-v4/what-is-yfx-v4)."),
    "dexs/allbridge-classic/index.ts": (146, "Считается сумма перевода исходного актива через мост за вычетом платы. Это строка Classic."),
    "dexs/beezie.ts": (169, "Игра и маркетплейс торгуют отдельными коллекционными предметами. SWAP выкупает такой предмет. [Правила игры](https://docs.beezie.com/our-offerings/the-claw) · [Маркетплейс](https://docs.beezie.com/our-offerings/marketplace)."),
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



# Действие, которое считает закреплённый адаптер. Категория бренда и величина
# оборота не участвуют в классификации. Неоднозначные ID разобраны отдельно.
_SOURCE_OPERATIONS = (
    ("spot", "382 4725 4834", "Обмен торгуемых токенов серии в AMM. Валюта пула и конечная валюта погашения требуют отдельной проверки."),
    ("spot", "5919 4764 2541 230", "Книга сопоставляет заявки покупки и продажи. Историческая запись не устанавливает текущий остаток предложения."),
    ("spot", "7984 7508 9436 7979 7982 6557 7644", "Записаны обмены за счёт конкретной кассы маркетмейкера. Права плательщика и актуальная цена проверяются для новой сделки."),
    ("spot", "2803 1447 2589 622 2954 9222", "Расчёт по котировке маркетмейкера. Для своего плательщика требуется действующее предложение."),
    ("spot", "5221 8322 8823 9267 7945", "Обмен исполняется на условиях ордера, подписанного намерения или котировки поставщика."),
    ("spot", "405 2116", "В учёте присутствует самостоятельная RFQ-ликвидность. В составной сделке объём стороннего AMM учитывается у исполнителя."),
    ("spot", "483", "Оборот vToken/WETH в собственном AMM NFTX, включая NFT-покупки через эти пулы. [Роли AMM](https://docs.nftx.io/core-contracts/nftxfeedistributorv3)."),
    ("spot", "5662", "Источник считает события Spot Swap в пулах InfinityPools."),
    ("provider", "8464", "LyraSwapRouter проводит обмены через Uniswap V2/V3/V4. Одна операция учитывается в обеих строках."),
    ("spot", "5458 4703 9172 3534 3620 4530 6616 7436 7955 8440 8860 9090 9400 3218 6246 3561", "Источник измеряет обмены активов в пулах этого семейства. Наличие резерва и плата на конкретном объёме проверяются отдельно."),
    ("spot", "6069 4983 9461 7447 4584 7935 8067 8865 9205 5195 6396 8838", "Покупки и продажи по кривой или в зарегистрированных пулах выпуска. После миграции действует площадка соответствующего пула."),
    ("spot", "4505 4557", "Источник учитывает оборот продажи токенов в пулах запуска Fjord."),
    ("spot", "3792 3833 5126 3036", "В исходной записи есть отдельная ветвь swap. Выпуск долей или позиции с плечом не добавляют к ней отдельную ликвидность."),
    ("spot", "7133", "Darkpool отдельно исполняет токеновые заявки против средств filler. В общий показатель также входят маржинальные операции."),
    ("spot", "5309 5734 6229", "Исходник учитывает обмены в AMM или на торговой кривой. Свежую цену и фактическую выдачу требуется подтвердить на конкретном рынке."),
    ("spot", "5103", "Проект указывает торговлю токенами в собственном dark pool на Base. [Интерфейс Renegade](https://renegade.fi/)."),
    ("provider", "8849 5408 7626 6049 5806 7214 8034 3336 8664 7768 8126 3106", "Источник учитывает спотовые сделки через терминал или маршрутизатор. Отдельный резерв поставщика этим оборотом не подтверждён."),
    ("provider", "3664", "Адаптер Primex выделяет SpotSwap через менеджер обмена, а также учитывает открытие и закрытие позиций. Маржинальный объём не увеличивает доступную спотовую глубину."),
    ("provider", "8811", "Торги пулами выпусков Peddles проходят через Uniswap V4. Один PoolId следует считать один раз."),
    ("provider", "8449", "WISE использует пару Uniswap V2. Профиль исходного AMM не является отдельным профилем Wise Token."),
    ("other", "4196 6650", "Брокер принимает распоряжение на покупку или продажу токенизированной бумаги. Нужны допуск владельца и подтверждённое исполнение. Сведения о выпуске не устанавливают прямую пару dShare/USDT."),
    ("provider", "7230", "Superposition исполняет обмен за счёт кошельков поставщиков. Получателю нужны согласованные условия и подпись поставщика."),
    ("provider", "5875", "Tea-Fi получает комиссию с routed swap. Из учтённой комиссии отдельный резерв своего пула не следует."),
    ("provider", "6092", "Источник обменов запрашивает статистику маршрутов Swaps.io по chainId. Нижележащая касса в ответе не определена."),
    ("provider", "5065", "В оборот входят исполненные swap и flashSwap через контракт Apex. Методика не устанавливает отдельный запас ликвидности Apex."),
    ("other", "6625", "Доли игрока ERC1155 обращаются по номеру и правилам площадки."),
    ("other", "6947 6390 8881", "Обмен стейблкоина на фиат. Криптоперевод и подтверждение фиатной оплаты проходят по разным каналам."),
    ("other", "7902 5181 5251 9287", "Предмет сделки — позиция по конкретному исходу. Его торговлю и погашение нельзя приравнять к обычной паре валют."),
    ("other", "3377 8512", "Запись Trade измеряет покупку и продажу внутренних социальных ключей по кривой. Независимый перевод ключа как валюты не установлен."),
    ("other", "4086 4944 5399", "Источник учитывает межсетевой обмен. Валюты и сроки подтверждения двух сетей различаются."),
    ("other", "4605 7350", "Торгуется право на поставку или заблокированные токены с вестингом. Передачу права и получение свободных валют проверяют отдельно."),
    ("other", "7986 6784", "Измерены средства аукциона или первичного размещения. Оплата, выделение токенов и возможность получить их на кошелёк могут происходить в разные моменты."),
    ("other", "4197", "Срочная книга кредитования. Позицию допускается токенизировать в ERC20. Отдельная пара вторичного рынка не установлена. [Правила книги](https://docs.secured.finance/fixed-rate-lending/overview)."),
    ("other", "364 5245", "Показатель учитывает выпуск и погашение токенов или разбиение и объединение позиций. Для вторичного обращения нужен самостоятельный рынок."),
    ("other", "8767 8292", "Оборот рассчитывается по приобретению NFT. Источник также выделяет выкуп ERC20, но первичный показатель не измеряет его как отдельный рынок."),
        ("unknown", "7607", "Rosetta измеряет поступления на router и начисляет расчётную комиссию. Проект маршрутизирует капитал между доходными стратегиями. Нужны выходной актив и подтверждение обмена в этой метрике. [Описание протокола](https://www.rosetta.sh/)."),
    ("other", "7822", "OTC между сетями. Источник считает первое раскрытие hashlock. Для внутрисетевого рынка требуются активы и сети обеих ног. [Описание расчёта](https://hashlock.markets/)."),
    ("unknown", "5140", "Elixir возвращает protocolDayData.volumeUSD без активов. Нужна привязка к рынку на Base и виду сделки."),
    ("unknown", "5648", "O.LAB возвращает суточную сумму API. Предмет сделки и способ передачи второй валюты не установлены."),
    ("unknown", "3509 5117", "Архивный модуль экспортирует breakdown.swap. Без сетевого определения импорта нельзя установить расчёт на Base."),
    ("unknown", "7648", "В карточке старой версии есть ссылка на каталог metric без зафиксированного исполняемого модуля. Кассу V0 нельзя вывести из размещения V1."),
    ("spot", "1700", "Сетевые пулы Wombat проверены отдельно от оборота. [Реестр и проверки Ethereum](../tests/data/first_run_dex_2026-11-26/wombat_source_ethereum_wallets.json.gz)."),
    ("unknown", "392", "Именованная секция wardenswap у общего helper содержит BSC. Для Ethereum нет подтверждённой сетевой привязки."),
    ("unknown", "2393", "В исходнике модуль traderjoe-v2 отделён от v2.1. Нужна привязка именно V2 к Ethereum."),
    ("unknown", "2756", "Ссылка Sharpe ведёт на архивный каталог. Нет установленного метода расчёта для Ethereum."),
    ("unknown", "6327", "Закреплённый адаптер считает swap на Solana. Расчёт на Ethereum требует отдельного подтверждения."),
    ("unknown", "6256", "Статистика /bridge для ETH даёт day_vol. Из числа нельзя определить, переводится ли тот же актив или выплачивается другой."),
    ("unknown", "1208", "Сетевой API OpenLeverage сообщает суммарную статистику. Доли спотовых обменов и кредитных позиций отдельно не видны."),
    ("unknown", "5514", "Индексатор Mantis сообщает дневной объём. Для Ethereum отсутствует идентичность входного актива, выходного актива и кассы."),
    ("other", "3316", "У tanX действует Spot Trading на StarkEx. Суточный API не устанавливает отдельную спотовую кассу в Ethereum L1. [Реестр StarkEx](https://docs.starkware.co/starkex/)."),
    ("spot", "646", "Сеть ETH Main Net и контракт управления пулами uTrade V2 опубликованы разработчиком. [Контроллер Ethereum](https://docs.unifiprotocol.com/utrade-v2/ethereum/unificontroller.sol)."),
    ("other", "4670", "C3 учитывает исполненные сделки книги ETH/USDC через API баров. Сеть базового токена не устанавливает физическую кассу Ethereum."),
    ("spot", "366", "В зафиксированном helper указано семейство AMM и сетевой источник DFX. Активность отдельного пула требует адресной проверки."),
)
_SOURCE_BY_ID = {source_id: (kind, reason) for kind, source_ids, reason in _SOURCE_OPERATIONS
                 for source_id in source_ids.split()}
if len(_SOURCE_BY_ID) != sum(len(ids.split()) for _, ids, _ in _SOURCE_OPERATIONS):
    raise ValueError("Дублируется классификация исходной строки DefiLlama")

COVERAGE_LABELS = {
    "spot": "спот", "provider": "спот через провайдера", "nonspot": "нет спота",
    "other": "иной предмет или способ расчёта", "unknown": "вид обмена не установлен",
}


def verified_coverage_profiles(network, configured, snapshot):
    """Проверяет сетевую роль каждого связанного профиля по коду на блоке снимка."""
    checked = {}
    for source_id, profile_id in COVERAGE_PROFILES[network].items():
        profile = configured.get(profile_id)
        if not profile:
            continue
        settings = profile["settings"]
        if profile["chain_id"] != snapshot["chain_id"] or profile["network"] != network:
            raise ValueError(f"{network}/{source_id}: сеть профиля {profile_id}")
        role = next(key for key in ("dex.factory", "dex.ekubo_core", "dex.pmm_pool", "dex.pool_manager",
                                    "dex.vault", "dex.stable_pools", "dex.quoter") if key in settings)
        addresses = settings[role] if isinstance(settings[role], list) else [settings[role]]
        if not addresses:
            raise ValueError(f"{network}/{source_id}: в {profile_id} отсутствует {role}")
        for address in addresses:
            code = snapshot["contracts"].get(address.lower(), {})
            if not code.get("bytes") or not code.get("keccak256"):
                raise ValueError(f"{network}/{source_id}: {profile_id} {role} {address} отсутствует на блоке снимка")
            if role == "dex.factory" and settings.get("dex.factory_runtime_hash", code["keccak256"]).lower() != code["keccak256"]:
                raise ValueError(f"{network}/{source_id}: хеш фабрики {profile_id} отличается от снимка")
        checked[source_id] = profile_id
    return checked


def classify_source(network, adapter, checked_profiles):
    """Класс операции, наличие профильной записи и проверяемое объяснение для строки сети."""
    source_id = str(adapter["id"])
    excluded = nonspot(network, adapter)
    if excluded:
        return "nonspot", None, excluded
    profile = COVERAGE_PROFILES[network].get(source_id)
    if profile:
        available = checked_profiles.get(source_id)
        label = f"спот · профиль `{profile}`" if available else "спот"
        return "spot", available, label
    kind, reason = _SOURCE_BY_ID[source_id]
    return kind, None, f"{COVERAGE_LABELS[kind]} · {reason}"

def describe(network,source_id,adapter,configured,candidates):
    path = adapter.get("path")
    revision="be9967c3f0d3ccd06c871bab0d16beb3304de719"
    repository=f"https://github.com/DefiLlama/dimension-adapters/blob/{revision}/"
    # Each entry explains what the cited adapter actually counts on this chain.
    # A terminal's or card programme's turnover is not a second claim on its suppliers' reserves.
    dimensions = {
        "dexs/o1-exchange/index.ts": (150, "Оборот терминала o1 вычислен по комиссии, умноженной на 100. Запись не задаёт адрес пула, резерв или firm quote. Цену может подтвердить только фактический источник обмена."),
        "dexs/bloom.ts": (73, "Bloom сопоставляет сделки нижележащих DEX с адресами роутеров терминала. Один и тот же обмен нельзя прибавить как отдельный остаток Bloom."),
        "dexs/axiom.ts": (125, "Axiom считает сделки нижележащих площадок через контракты терминала. В этой строке нет отдельной кассы Axiom."),
        "dexs/aveai/index.ts": (162, "Ave.ai измеряет обмены через нижележащие DEX, включая исполненные лимитные заявки. Для глубины учитывается источник фактической оплаты каждой ноги."),
        "dexs/p2pme.ts": (1, "выплата USDC покупателю после исполнения заявки на покупку за фиат. Для этой сделки требуется подтверждение фиатной ноги."),
        "dexs/zkp2p.ts": (1, "сумма USDC по исполненным платёжным намерениям в индексе Peer. Для этой сделки требуется подтверждение фиатной ноги."),
    }
    if (network, source_id, path) == ("base", "2116", "dexs/0x-rfq.ts"):
        return ("На H=62262756 прочитаны текущий и предыдущий Settler реестра 0x для функции 2. "
                "За 43200 блоков у этих адресов найдено 118 маркеров в 107 квитанциях. У 92 ног "
                "условия из calldata совпали с двумя фактическими переводами между Settler и одним из четырёх maker. "
                "В этих квитанциях нет операций Transfer с Base USDT; отдельный поиск входящих и исходящих "
                "переводов именно этого токена для четырёх maker за 30 суток также пуст. Для нового обмена "
                "требуется действующая котировка API для своего кошелька и нужной пары. "
                "`T-SOURCE-base-2116` [Реестр](https://github.com/0xProject/0x-settler#how-do-i-find-the-most-recent-deployment) · "
                "[Квитанции и интервалы](../tests/data/first_run_dex_2026-11-26/zeroex_base_payments.json.gz).")
    if path in dimensions:
        line, reason=dimensions[path]
        return f"{reason} [Методика]({repository}{path}#L{line})."
    basin={
        ("base","5458","dexs/pinto/index.ts"):("pinto-exchange-base","pinto_base"),
        ("ethereum","4703","dexs/basin/index.ts"):("basin-wells-ethereum","basin_ethereum"),
    }.get((network,source_id,path))
    if basin:
        profile,stem=basin
        if profile not in configured:
            return None
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
