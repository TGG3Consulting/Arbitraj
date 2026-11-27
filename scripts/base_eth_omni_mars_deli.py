"""Сведения о фактическом реестре, ставках и выплатах из архивов общего справочника."""
import gzip
import json
from pathlib import Path

DATA = Path("tests/data/first_run_dex_2026-11-26")


def snapshot(name):
    return json.loads(gzip.decompress((DATA / name).read_bytes()))


def sectorone(network):
    output=["#### SectorOne / DLMM King", "",
            "Обе версии исполняют ERC-20 через прямой LBRouter с проверкой реального прироста выходного токена или через V5. "
            "В версии 2.0 прямой путь использует положительный binStep. У представленного как 2.2 роутера значение версии пути 2 выбирает собственную фабрику. "
            "Обе стороны учитывают комиссию конкретного бина и изменение волатильности. Движок проверяет пары по getAllLBPairs и LBPairCreated. "
            "[Адреса проекта](https://docs.sectorone.xyz/info/smart-contracts.md).", "",
            "| Версия | Зарегистрировано / с двумя резервами | Проверенный пул | Вход → выплата → возврат, в атомах | Газ Router / V5, покупка и продажа |",
            "|---|---:|---|---|---|"]
    profiles=json.loads(Path("config/first_run_dex.json").read_bytes())
    aliases={p["settings"]["dex.indexer_id"]:p["id"] for p in profiles if p["network"]==network and "dex.indexer_id" in p["settings"]}
    for v in ("2.0","2.2"):
        file=f"sectorone-{v.replace('.','')}-{network}.json.gz"
        archive=snapshot(file)
        d,receipt=archive["directory"],archive["settlement"]
        indexer="sectorone-base" if network=="base" and v=="2.2" else f"sectorone-v{v}-{network}"
        scope=aliases.get(indexer,indexer)
        output.append(f"| Индексатор `{indexer}` | Ключ настройки `{scope}` | | | |")
        markets=d["markets"]
        count=sum(all(map(int,r["reserves"])) for r in markets)
        direct=[x for x in receipt["cases"] if x["mode"]=="router"]
        v5=[x for x in receipt["cases"] if x["mode"]=="v5"]
        for a,b in zip(direct,v5,strict=True):
            back=a.get("sell",{})
            output.append(f"| {v} | {len(markets)} / {count} | `{receipt['pool']}` | {a['buy']['input']} → {a['buy']['net']} → {back.get('net','отказ')} | "
                          f"{a['buy']['gas']} / {b['buy']['gas']}, {back.get('gas','—')} / {b.get('sell',{}).get('gas','—')} |")
        output.append(f"| [Адреса, исходные вызовы и выплаты {v}](../tests/data/first_run_dex_2026-11-26/{file}) | | | | |")
    v2=snapshot(f"sectorone_v2_{network}.json.gz")
    output.extend(["", f"У отдельной фабрики постоянного произведения `{v2['factory']}` на контрольном блоке нет пар. "
                   f"Роутер `{v2['router']}` возвращает этот адрес через factory(). "
                   "В изолированном блоке на этой фабрике создана и профинансирована пара. "
                   "При ставке 0,30% и после изменения владельцем до 0,40% проверены по восемь выплат "
                   "на кошелёк (роутер и V5, два объёма в обе стороны). Для каждой выплаты проверены минимум на атом выше и истёкший срок. "
                   f"[Выплаты](../{DATA}/sectorone_v2_{network}.json.gz)."])
    if network=="ethereum":
        output.extend(["","У версии 2.0 на исходном блоке все восемь пар имели минимум одну пустую сторону. "
                       "В WETH→токен покупка внесла WETH в исходно пустую сторону и позволила обратный обмен. "
                       "У версии 2.2 меньшая покупка выплатила один атом токена, обратная нога на нём отклонилась."])
    return output+[""]


def paragraphs(network):
    return sectorone(network)+other_markets(network)


def other_markets(network):
    if network == "ethereum":
        mars = snapshot("marswap-ethereum.json.gz")
        rows = [r for r in mars["markets"] if "settlements" in r]
        samples = []
        for row in rows:
            order = next(c for c in row["settlements"]["cases"] if c["mode"] == "percentage")
            buy, sell = order["buy"], order["sell"]
            samples.append(f"| `{row['pool']}` | {buy['input']} → {buy['net']} → {sell['net']} | {sell['nominal']} |")
        _, lp, platform, flat = mars["terms"]
        return [
            "#### MarSwap на Ethereum", "",
            f"У фабрики `{mars['factory']}` зарегистрировано {mars['markets_count']} CREATE2-пар. "
            f"На блоке {mars['height']} у двух рынков ненулевые резервы, остальные три пусты. "
            f"Ставка обычного обмена равна ({lp} + {platform}) / 10 000 из `lpInfo()`. "
            f"Оплата {flat} wei ETH на один шаг отключает процентную ставку у проверенного Router02. "
            "На момент снимка это дороже совокупных 115 301 178 451 469 wei WETH в обеих активных парах. "
            "Для одиночной сделки между ERC20 сравниваются процентный Router02, V5 и Router02 с подписанной положительной платой ETH. Расчёт чистого результата включает отдельный расход ETH, газ обмена и разрешений. Перед подписанием сверяются ставка, код и регистрация пары, минимальный приход плательщику и его расход. В архиве две пары дали восемь выплат в фиксированном режиме. На продажах у вариантов отличались входные объёмы после предшествующих покупок.",
            "", "| Контракт пары | Вход WETH, фактическая покупка токена, обратная выплата WETH (атомы) | Расчётная обратная выплата по резервам до налога (wei) |",
            "|---|---:|---:|", *samples, "",
            "Проверены два размера, оба направления и три режима в каждой из двух пар. Процентный роутер и V5 дали одинаковую чистую выплату. "
            "Налог токена зависит от направления. Сигнал использует фактическую выплату и полное списание выбранного кошелька. "
            "[Запросы, отказы и остатки](../tests/data/first_run_dex_2026-11-26/marswap-ethereum.json.gz).",
            "", "Размещение DOCSWAP и число зарегистрированных пар приведены по отдельному каноническому чтению ниже.",
            ""
        ]
    omni = snapshot("omni-v2-base.json.gz")
    mars = snapshot("marswap-base.json.gz")
    example = next(e for e in omni["examples"] if "wallet" in e)
    wallet = example["wallet"]
    deals = []
    for case in wallet["cases"]:
        direct, executor = case["native"], case["v5"]
        totals = [str(int(c[leg]["results"][-3]["gasUsed"], 16)) for c in (direct, executor) for leg in ("buy","sell")]
        deals.append(f"| {direct['buy']['input']} → {direct['buy']['output']} → {direct['sell']['output']} | "
                     f"{totals[0]} / {totals[1]} | {totals[2]} / {totals[3]} |")
    return [
        "#### Omni Exchange V2 и MarSwap на Base", "",
        f"У OmniFactory `{omni['factory']['address']}` подтверждены {len(omni['markets'])} CREATE2-пар. "
        f"Фиксированная комиссия равна 25/10 000; проверенный хеш создания `{omni['init_hash']}`. "
        f"В паре `{example['pool']}` кошелёк `{wallet['owner']}` выполнил защищённые обмены через "
        f"Universal Router `{omni['router']['address']}`, его собственный Permit2 `{wallet['permit2']}` и V5. "
        "При первом запуске доступны V5 и стандартный путь через Omni Universal Router с проверкой встроенных адресов. Строгий минимум выше выплаты на один атом и истёкший срок отклонены в каждом направлении. "
        "У других зарегистрированных пар результат зависит от токенов, суммы и состояния своей пары.",
        "", "| Вход WETH → токен → WETH (атомы) | Газ команды Universal Router, покупка/продажа | Газ команды V5, покупка/продажа |",
        "|---:|---:|---:|", *deals, "",
        "Газ разрешений оплачивается отдельно при недостаточном allowance. "
        "[Исходник роутера с привязкой Permit2 и фабрики](https://sourcify.dev/server/v2/contract/8453/0x423fae8ee58833f283e8845479bd705514110ec3?fields=all) · "
        "[Каталог и фактические выплаты](../tests/data/first_run_dex_2026-11-26/omni-v2-base.json.gz).",
        "", f"У фабрики MarSwap `{mars['factory']}` на блоке {mars['height']} метод `allPairsLength()` вернул {mars['markets_count']}. "
        f"Её `INIT_CODE_PAIR_HASH()` совпадает с Ethereum: `{mars['init_hash']}`. Адреса фабрик этих сетей различаются. На этом блоке код штатного MarsRouter подтвердил обе роли factory/WETH. В изолированном блоке создана пара официальной фабрики, в которую внесена тестовая ликвидность. Два размера в обоих направлениях дали 12 выплат кошельку. Каждая отказала при минимуме на атом выше и истёкшем сроке. При положительной плате 40 000 000 000 000 wei сравниваются фиксированный Router02 и процентные способы. В отдельном контроле с нулевой фиксированной платой V5 с 20 базисными пунктами провёл четыре обмена. Router02 и V5 без процента откатили восемь попыток проверки инварианта пары. Исходный реестр пуст, оборот или прибыль на Base из этого опыта не следуют. [Транзакции симуляции](../tests/data/first_run_dex_2026-11-26/marswap_base_simulated.json.gz). "
        "[Каноническая запись](../tests/data/first_run_dex_2026-11-26/marswap-base.json.gz).",
        "", "В опубликованных материалах проекта DeJungle указатель на фабрику отсутствует. "
        "Нужны адрес развёртывания на Base, регистрация рынка и исполнение в кошельке.",
        "", "#### Deli Swap на Base", "",
        "События Initialize у Uniswap V4 PoolManager доказывают два поколения. Семь сохранённых PoolId используют "
        "DeliHookConstantProduct `0x95afbc0fccf974b41380f24e562f15b6dd90fac8`. Состояние каждого рынка хранится в getReserves(poolId) самого хука. "
        "У четырёх из них остаток wBLT на блоке снимка составлял от 5 до 1081 атома. "
        "Четыре отдельных рынка (wBLT/USDC на двух шагах цены, wBLT/cbBTC, wBLT/BMX) используют DeliHook "
        "`0x570a48f96035c2874de1c0f13c5075a05683b0cc` с концентрированной ликвидностью и управляемой ставкой. "
        "Для CL обычный кошелёк на двух объёмах в обоих направлениях успешно обменял wBLT/USDC (шаг 40), wBLT/cbBTC и wBLT/BMX; на отдельном wBLT/USDC (шаг 60) обе исходные котировки из wBLT отклонились. При отдельном старте с 2 и 100 000 атомов USDC оба кошельковых пути выплатили 1 009 638 069 109 и 99 488 139 484 180 048 атомов wBLT. При входе 1 атома USDC Quoter вернул 340282366920938463463374607431766854381, а отправка с минимумом в 1 атом wBLT на обоих путях отклонилась с нулевым движением балансов. [Котировки и остатки CL](../tests/data/first_run_dex_2026-11-26/deli_base_cl_wallets.json.gz). Каждая модель вызывает внешние контракты учёта комиссии и вознаграждений. "
        "В семи пулах constant-product получены 20 полных циклов Router/V5 и 46 исполненных ног из 56; на четырёх пылевых "
        "рынках меньший размер округляется до нуля хотя бы в одну сторону. Живой снимок связывает каждый ключ с кодом хука, "
        "кодом feeProcessor и обеих gauges. После внешних вызовов программа разрешает отдельный путь с подтверждённым минимумом. "
        "[Выплаты кошельку и газ по каждому размеру](../tests/data/first_run_dex_2026-11-26/deli_base_wallets.json.gz) · "
        "[Initialize постоянного произведения](../tests/data/first_run_dex_2026-11-26/deli_base_initializations.json) · "
        "[Initialize концентрированной ликвидности](../tests/data/first_run_dex_2026-11-26/deli_base_cl_initializations.json) · "
        "[Исходники проекта](https://github.com/useboardwalk/deli-swap-contracts).", ""
    ]
