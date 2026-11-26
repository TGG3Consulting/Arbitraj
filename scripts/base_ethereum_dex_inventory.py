"""Восстанавливает раздел DEX общего справочника из снимков исследования 26.11.2026 и профилей сборки.

Запуск из корня репозитория: python scripts/base_ethereum_dex_inventory.py ../outputs
Использует только файлы. Другие разделы документа сохраняются.
"""
import argparse
import hashlib
import json
from pathlib import Path


def read(path):
    """Возвращает данные JSON в UTF-8. Неполный или повреждённый снимок прерывает генерацию."""
    return json.loads(path.read_text(encoding="utf8"))


def cell(value):
    """Экранирует разделители столбцов и переносы строки из внешнего каталога."""
    return str(value).replace("|", "\\|").replace("\n", " ")


def addresses(items):
    """Формат списка адресов или явная пометка при отсутствии значения."""
    return " ".join(f"`{x}`" for x in items) or "не установлены"


def tables(network, snapshots, profiles):
    """Строит таблицы одной сети. Сохраняет исходные идентификаторы каждой площадки и наблюдения."""
    base = network == "base"
    p = snapshots / f"{network}-integration-2026-11-26" / "sources"
    evidence = "base-contract-evidence" if base else "dex-contract-evidence"
    names = ["index", "sample-pools", "factory-probes", "volumes", "llama-adapters",
             "velora", "family-coverage-review", evidence]
    data = {name: read(p / (name + ".json")) for name in names}
    gt = data["index"]
    llama = data["volumes"]
    va = data["velora"]
    block = data["family-coverage-review"]
    extra_path = Path("tests/data/first_run_dex_2026-11-26/catalog_recheck.json")
    refresh = read(extra_path).get(network, {}) if extra_path.exists() else {}
    refreshed = refresh.get("pages", {})
    volume = refresh.get("volume", {})
    title = "Base" if base else "Ethereum"
    out = [f"### {title} · DEX", "",
           f"Снимок GeckoTerminal от {gt['download_utc']}, {len(gt['rows'])} идентификаторов. "
           f"DefiLlama от {llama['download_utc']}, {len(llama['rows'])} строк. "
           f"Сетевые разделы Velora — {len(va['rows'])}. Строки разных источников могут относиться к одному пулу. "
           f"Контрольный блок {block['block']}, `{block['hash']}`.", "",
           "#### Значения для экрана «Первый запуск»", "",
           "| ID DEX | Настройки области DEX | Основание |", "|---|---|---|"]
    configured = {r["id"]: r for r in profiles if r["network"] == network}
    matched = set(configured).intersection(gt["rows"])
    out[2] += (f" Из {len(gt['rows'])} строк GeckoTerminal адресные профили заданы для {len(matched)}, "
               f"остаются открытыми {len(gt['rows']) - len(matched)}. Ещё {len(configured) - len(matched)} "
               "внутренних ключей обозначают отдельные фабрики и семейства.")
    for id, row in configured.items():
        fields = []
        for k, v in row["settings"].items():
            if isinstance(v, list):
                values = [f"{x}{{суффикс}}" for x in v] if k == "dex.sub_dex" else v
                v = ", ".join(values) or "[]"
            elif isinstance(v, bool):
                v = "да" if v else "нет"
            fields.append(f"`{k} = {v}`")
        params = "<br>".join(fields)
        out.append(f"| `{id}` | {params} | {row['source']} |")
    metric_path = Path(f"tests/data/first_run_dex_2026-11-26/metric_{network}.json")
    if metric_path.exists():
        metric = read(metric_path)
        out.extend(["", f"Metric OMM · проверено {metric['registered_samples_read']} регистраций на закреплённом блоке "
                    f"{metric['block']}. Для трёх пулов котировка повторена штатным кодом V5 при двух объёмах "
                    "и двух внешних отправителях. Проверены обе стороны, зачисление на кошелёк, остатки на "
                    "исполнителе и отказ minOut + 1. Пригодность оракула, расширений и реального кошелька "
                    f"повторно проверяется перед обменом. [Снимок и результаты](../{metric_path.as_posix()})."])
    metadex = Path(f"tests/data/first_run_dex_2026-11-26/metadex_{network}.json")
    if metadex.exists():
        observed = read(metadex)
        active = sum(int(p["liquidity"]) > 0 for p in observed["pools"])
        cases = len(observed["v5"]["cases"])
        mev = ("У модуля приоритетной комиссии множитель равен нулю"
               if network == "base" else "Крючок возвращает нулевой адрес модуля MEV")
        out.extend(["", f"Aero MetaDEX Slipstream · блок {observed['block']}. Фабрика предоставляет семь шагов тика. "
                    f"Проверены DynamicSwapFeeHook, реализация пула, библиотека и официальный Metaquoter. "
                    f"{mev}. Скидка может зависеть от вызывающего. Для Metaquoter и V5 в выборке первая комиссия "
                    f"совпала с расчётом по наблюдениям TWAP. Горизонт профиля — {2*observed['slot_seconds']} с. "
                    f"Для {active} активных пулов записаны {cases} обменов V5 в обе стороны на двух размерах. "
                    "Котировка совпала с выходом по событию Swap и приростом баланса. "
                    "При увеличении минимального выхода на одну младшую единицу симуляция отклонила каждый обмен. "
                    + ("У ETH-пула WETH/USDC с шагом 10 на этом блоке нулевая активная ликвидность. "
                       if network == "ethereum" else "")
                    + f"[Исходники, модуль, комиссии и симуляции](../{metadex.as_posix()})."])
    from_dir = f"../tests/data/executor_v5_{network}_2026-11-26"
    out.extend(["", f"Uniswap V4 · два пула USDT/USDC, hook 0. Для каждого выполнены симуляции "
                f"покупки в размере 1 и 100 USDT и обратной продажи полученного USDC. На всех восьми шагах сети "
                f"ArbExecutor V5 вернул сумму квотера и кредита события Swap с нулевыми остатками обоих токенов. "
                f"В отдельной проверке значение minOut = квота прошло для каждой стороны и размера, а квота + 1 дала откат. "
                f"[Граница minOut]({from_dir}/min_out.json). Для обычного кошелька нужен его собственный исполнитель. [Запись V5]({from_dir}/README.md) · "
                f"[Суммы и результаты]({from_dir}/summary.json)."])
    market_file = f"tests/data/first_run_dex_2026-11-26/balancer_{network}.json"
    if Path(market_file).exists():
        snapshot = read(Path(market_file))
        market = snapshot["market"]
        factories = snapshot["v3_pause_scan"]["factories"]
        total = sum(f["pool_count"] for f in factories)
        paused = sum(f["paused"] for f in factories)
        counts = [sum(row["version"] == v and row["status"] == "quoted" for row in market) for v in (2, 3)]
        out.extend(["", f"Balancer · прямая и обратная котировки для {counts[0]} пулов V2 через Vault.queryBatchSwap "
                    f"и {counts[1]} пулов V3 через Router.querySwapSingleTokenExactIn. "
                    "У каждого установлена фабрика по isPoolFromFactory и проверены токены хранилища. "
                    "Расчёт использует одну стотысячную от баланса входного токена в пуле. Для V3 разрешение "
                    "MEV Capture требует оценки сетевых чаевых, QuantAMM — подтверждения размещения собственной фабрики. "
                    f"На закреплённом блоке среди {total} пулов {len(factories)} фабрик V3 на паузе {paused}, "
                    f"паузу не установили у {total-paused} (isPoolPaused = false); isVaultPaused = false. "
                    "По утверждённому BIP-928 пауза Vault V3 запланирована на 30.11.2026. "
                    f"[Вызовы и ответы](../{market_file}) · "
                    "[Решение Balancer](https://forum.balancer.fi/t/bip-928-orderly-winddown-of-balancer-and-distribution-of-the-treasury/7107/23)."])
    curve_file = f"tests/data/first_run_dex_2026-11-26/curve_{network}.json"
    if Path(curve_file).exists():
        groups = read(Path(curve_file))["families"]
        quoted = {k: sum(p["status"] == "quoted" for p in f["pools"]) for k, f in groups.items()}
        out.extend(["", f"Curve · по двум направлениям и двум размерам записаны get_dy для "
                    f"{quoted['stable-ng']} пулов StableSwap-NG, {quoted['twocrypto']} Twocrypto-NG "
                    f"и {quoted['tricrypto']} Tricrypto-NG. Фабрика подтвердила монеты, каждый пул сообщил "
                    "версию. Для Twocrypto две версии математики совпадают по байткоду с ранее проверенными. "
                    + ("Tricrypto использует тот же байткод математики, что проверялся на Arbitrum. "
                       if base else "У Tricrypto математика скомпилирована Vyper 0.3.9. Текст исходника "
                       "в Sourcify совпал с проверенной версией после трёх директив компилятора в эталоне. ")
                    + "Метапулы StableSwap-NG требуют отдельного профиля. "
                    + f"[Пулы, вызовы, квоты и сверка кода](../{curve_file})."])
    legacy = f"tests/data/first_run_dex_2026-11-26/curve_plain_{network}.json"
    if Path(legacy).exists():
        info = read(Path(legacy))
        proven = sum(r["status"] != "unproven" for r in info["templates"])
        out.extend(["", f"Curve StableSwap Plain · {proven} из {len(info['templates'])} реализаций "
                    "из справочника Curve SDK прошли сверку по байткоду или исходникам get_dy, exchange, "
                    "вызываемым ими функциям и числовым константам. Четыре действующих пула проверены "
                    "по фабрике и двум размерам get_dy в обоих направлениях. "
                    f"[Реализации, основания и результаты](../{legacy})."])
    ekubo = f"tests/data/first_run_dex_2026-11-26/ekubo_{network}.json"
    if Path(ekubo).exists():
        out.extend(["", "Ekubo V3 · у двух пулов без расширений ключ установлен по событию PoolInitialized ядра. "
                    "Восемь Router.quote подтверждают полное списание в обе стороны для нативной пары и пары ERC-20. "
                    + ("У проверенного ETH/USDC лимит в сторону ETH → USDC на этом блоке составил всего "
                       "20 921 872 214 wei. Более крупные пробы сохранены с частично принятым входом. "
                       if base else "")
                    + "Область профиля ограничена пулами без расширений. Размер окна dex.ekubo_bitmap_words, "
                    "буфер подписки dex.ekubo_hub_buffer и доступ к отправке выбирает оператор. "
                    f"[Ключи, тики, квоты и события](../{ekubo}) · "
                    "[Размещения Ekubo](https://docs.ekubo.org/reference/contracts/evm-v3/)."])
    fermi = Path("tests/data/first_run_dex_2026-11-26/fermi_ethereum.json")
    if not base and fermi.exists():
        data_fermi = read(fermi)
        rows = sum(len(v["cases"]) for v in data_fermi["simulations"].values())
        out.extend(["", f"FermiSwap · три версии подтверждены квитанциями Ethereum и переводами токенов. "
                    f"На блоках 26 494 352–26 494 988 все 131 наблюдение относятся к "
                    f"\x60{data_fermi['router']}\x60. В слоте 15 740 677 реальные кадры Titan и BuilderNet "
                    f"дали {rows} проверенных исполнений WETH ↔ USDT с точным минимумом и откатом при минимуме "
                    "на единицу выше. Без обновления оракула и в следующем блоке котировки вернули SP. "
                    "Сумма зависит от выбранного строителя блока. Для торговли требуются отдельный канал "
                    "этого строителя на точный блок, проверка остаточного времени и восстановление исходного намерения. "
                    f"[Блоки, котировки, движения токенов](../{fermi.as_posix()}) · "
                    "[Адреса и порядок исполнения Titan](https://docs.titanbuilder.xyz/propamms)."])
        slot_receipts = fermi.with_name("fermi_slot_receipts.json")
        if slot_receipts.exists():
            wrapped = read(slot_receipts)
            weth = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
            measured = wrapped["cases"]
            fmt = lambda n: f"{n:,}".replace(",", " ")
            prices = {int(row["receipt"]["effectiveGasPrice"], 16) for row in measured}
            assert len(prices) == 1
            price = prices.pop()
            modeled = [row["swap_gas"] * price for row in measured]
            out.extend(["", "FermiSlotExecutor · изолированная сборка GitHub, слот 15 740 677. "
                        "Расход обмена на кадрах обоих строителей в единицах газа.", "",
                        "| Направление | Точный вход, Titan / BuilderNet | Точный выход, Titan / BuilderNet | Первое разрешение |",
                        "|---|---:|---:|---:|"])
            for is_weth, title in [(True, "WETH → USDT"), (False, "USDT → WETH")]:
                side = [r for r in measured if (r["input"] == weth) == is_weth]
                costs = {(r["builder"], int(r["amount_specified"]) > 0): r["swap_gas"] for r in side}
                cells = [" / ".join(fmt(costs[builder, exact_in]) for builder in ("Titan","BuilderNet"))
                         for exact_in in (True, False)]
                approvals = sorted({r["approval_gas"] for r in side})
                out.append("| " + " | ".join([title, *cells, "–".join(fmt(n) for n in approvals)]) + " |")
            eth_bounds = "–".join(f"{wei/10**18:.8f}".replace(".", ",") for wei in (min(modeled),max(modeled)))
            out.extend(["", f"Runtime Keccak `{wrapped['runtime_keccak']}`. "
                        f"При цене контрольной транзакции {fmt(price)} wei/gas модель обмена составила {eth_bounds} ETH. "
                        "Для первой сделки отдельно оплачивается разрешение. Маршрут приложения использует действующее разрешение "
                        "плательщика и проверяет параметры в карточке Ethereum. Размещение контракта и приёмка реальной сделки требуют решения владельца. "
                        f"[Восемь квитанций симуляции](../{slot_receipts.as_posix()}) · [Параметры](SETTINGS.md)."])
        coverage_path = fermi.with_name("fermi_builder_coverage.json")
        windows_path = fermi.with_name("fermi_market_windows.json")
        if coverage_path.exists() and windows_path.exists():
            coverage = read(coverage_path)
            windows = read(windows_path)
            stats = coverage["summary"]
            out.extend(["", "В коротком окне 25.11 с 19:41:23 до 21:49:11 UTC проверены хеши и родство "
                        f"всех {coverage['canonical_blocks']} блоков Ethereum. Среди доставленных payloads "
                        f"{stats['Titan']['blocks']} блока атрибутированы Titan, {stats['BuilderNet']['blocks']} BuilderNet, "
                        f"{stats['other']['blocks']} другим строителям. Для {stats['ambiguous']['blocks']} блоков "
                        f"реле сообщили ключи разных групп, для {stats['unattributed']['blocks']} в ответах нет совпадения. "
                        "Из 131 события FermiSwap на два подтверждённых источника пришлись 65 и 22. "
                        f"[Канонические хеши и отчёты семи реле](../{coverage_path.as_posix()}).", "",
                        "| Окно Relayscan | Блоки в таблице строителей | Titan | BuilderNet |",
                        "|---|---:|---:|---:|"])
            for label,k in [("24 часа","24h"),("7 дней","7d")]:
                report = windows["builders"][k]
                row = report["selected"]
                titan, buildernet = row["Titan (titanbuilder.xyz)"], row["BuilderNet"]
                out.append(f"| {label} | {report['table_blocks']} | {titan['blocks']} ({titan['percent']}) | "
                           f"{buildernet['blocks']} ({buildernet['percent']}) |")
            out.extend(["", "Relayscan группирует блоки своей выборки по меткам extra_data. Окна заканчиваются "
                        f"на слоте {windows['builders']['7d']['as_of_slot']}. По опубликованным рядам DefiLlama "
                        "за закрытые сутки 25.11 объём Fermi составил 61,91 млн USD из 1,395 млрд USD на Ethereum "
                        "(4,44%), за 19–25.11 — 378,33 млн USD из 11,472 млрд USD (3,30%). "
                        "Для оценки прибыли нужны исполнимый объём и цены обеих сторон сделки, допуск к слоту, "
                        "вероятность включения, газ и комиссии. "
                        f"[Счётчики строителей и дневные объёмы](../{windows_path.as_posix()})."])
    pancake = "tests/data/first_run_dex_2026-11-26/pancake_stable_ethereum.json"
    if not base and Path(pancake).exists():
        out.extend(["", "PancakeSwap StableSwap · два пула из официального API составляют полный реестр Ethereum "
                    "на контрольном блоке. Фабрика установлена по событию создания каждой пары и ответу getPairInfo. "
                    "Хеш Keccak-256 исходника PancakeStableSwapTwoPool.sol совпал с эталоном решения 471 у обоих пулов, is_killed = false. "
                    "Для восьми входов отдельно проверены get_dy и фактическая дельта balanceOf после exchange "
                    "в eth_simulateV1 с реальными балансами. При входе 4449 минимальных единиц WBTC get_dy "
                    "вернул 4456, exchange перевёл 4455 минимальных единиц mBTC: контракт округляет комиссию "
                    "до перевода в единицы монеты при обмене и после перевода в get_dy. У всех восьми обменов "
                    "min_dy на единицу выше выплаты дал защитный откат. "
                    f"[Реестр и котировки](../{pancake}) · "
                    "[Исторические обмены](../tests/data/first_run_dex_2026-11-26/pancake_stable_exchange_ethereum.json)."])
    kyber = f"tests/data/first_run_dex_2026-11-26/kyber_{network}.json"
    if Path(kyber).exists():
        out.extend(["", "KyberSwap Elastic V2 · фабрика подтвердила три пула и десять уровней комиссии. "
                    "В каждом пуле прочитаны все инициализированные тики через TicksFeesReader частями "
                    "по 1000; на двух объёмах в обе стороны QuoterV2 вернул полный usedAmount и положительный "
                    "returnedAmount. Проверены initcode фабрики, код размещённого пула и 13 исходников "
                    "из закреплённой ревизии Kyber. "
                    + ("Кандидаты уровней получены из выборки и событий Ethereum, каждый подтверждён собственным реестром Base. "
                       if base else "У Classic проверены отдельные фабрики динамической и постоянной комиссии: "
                       "у каждой по два пула с четырьмя котировками getAmountsOut. ")
                    + f"[Реестры, код, тики и котировки](../{kyber})."])
    solidly = Path(f"tests/data/first_run_dex_2026-11-26/solidly_v3_{network}.json")
    if solidly.exists():
        sl = read(solidly)
        markets = sl["markets"]
        fills = [case for m in markets for case in m["cases"] if "results" in case]
        full = sum(c["dx"] == c["taken"] for c in fills)
        out.extend(["", f"Solidly V3 · фабрика `{sl['factory']}`, связь getPool(token0, token1, int24 spacing) "
                    f"подтверждена для {len(markets)} пулов. slot0() возвращает цену, тик, текущую комиссию и признак "
                    f"разблокировки. Комиссии в миллионных долях по порядку пулов снимка: "
                    f"{', '.join(str(m['fee']) for m in markets)}. Контракт публикует SetFee при смене комиссии. "
                    f"В симуляциях на реальных балансах {len(fills)} обменов через swap с шестью аргументами "
                    f"совпали с quoteSwap по обоим токенам, из них {full} списали весь запрошенный вход. "
                    "Для каждого проверены точный minOut, отказ AL при minOut + 1, TO при истёкшем сроке и отказ "
                    "от маршрута V5 с обратным вызовом Uniswap. Прямое разрешение пулу использовано только в симуляции. "
                    "Вид solidly_v3 включает чтение и сигналы. Для вида solidly_v3_module владелец указывает "
                    "свой развёрнутый адрес в dex.router и включает dex.pool_enabled. Требуется приёмочный "
                    "хеш runtime в справочнике конкретной сборки для chainId; текущие байты проверяются при чтении "
                    "и перед сборкой разрешений. Принятая нога бандла ограничена одним пулом, кошелёк разрешает "
                    "списание модулю, модуль выдаёт пулу разрешение ровно на вход вызова. "
                    f"[Фабрика и байткод, вызовы, балансы](../{solidly.as_posix()}) · "
                    "[Адреса протокола](https://docs.solidly.com/resources/contract-addresses.html)."])
        if not base:
            out.extend(["", "Ethereum · у USDC/WETH из бюджета 1 WETH пул списал 0,219192077251805298 WETH "
                        "и выплатил 204,192073 USDC. У USDC/USDT без ограничения газа quoteSwap на 10 USDT "
                        "показал расход 8,137669 USDT и выход 8,113045 USDC. При пределе 16 777 216 gas вызов "
                        "превысил лимит. Для DEFROGS/WETH проверены обе стороны обмена на двух размерах."])
    lista = "tests/data/first_run_dex_2026-11-26/lista_ethereum.json"
    lista_stable = "tests/data/first_run_dex_2026-11-26/lista_stable_ethereum.json"
    if not base and Path(lista).exists():
        out.extend(["", "Lista DEX V3 · три пула официальной фабрики Ethereum и семь разрешённых уровней комиссии "
                    "установлены по событиям и обратным запросам. У всех трёх пулов код 31 зависимой единицы Solidity "
                    "совпал с Uniswap V3 после учёта путей импорта и переименования интерфейсов. Для трёх пар "
                    "12 обменов через V5 совпали с двумя квотерами и изменением балансов. Повышение минимума на "
                    "единицу отменило каждый обмен. На паре USDT/USDS при заявке 100 USDT пул взял только "
                    "50,415023 USDT. При заявке 100 USDS он взял 48,517287003583698682 USDS. Официальный квотер "
                    "возвратил положительный выход в обоих случаях. Профиль использует VirtualQuoter с проверкой "
                    "полноты списания. " f"[Фабрика, тики, исходники, котировки и обмены](../{lista})."])
    if not base and Path(lista_stable).exists():
        out.extend(["", "Lista StableSwap · в официальном каталоге 685 кредитных рынков. Из них 22 рынка "
                    "Ethereum с умным обеспечением используют 11 уникальных пулов. Три принадлежат ListaV3, "
                    "остальные восемь перечислены фабрикой StableSwap. У всех восьми paused() = true, в обе "
                    "стороны exchange вернул EnforcedPause. Вид lista_stableswap_proxy проверяет EIP-1967 на "
                    "каждом чтении. У всех восьми текущая реализация 0xefcb1be95a933ba1918f035f90e6dba1c1d778da. "
                    "Фабрика задаёт другую реализацию 0x86d2946bd6c807e797e8eec0d632931dc337dea3 для будущих пулов. "
                    "На активном блоке 26 009 037 та же реализация USDC/USDT прошла четыре обмена от держателей "
                    "и четыре защитных отказа. За 1 USDT get_dy показал 999 826 минимальных единиц USDC, "
                    "exchange перечислил 999 825. На следующем блоке зарегистрировано событие Paused. "
                    f"[Реестр, причина паузы, исходники и исторические обмены](../{lista_stable})."])
    quickswap_v2 = Path("tests/data/first_run_dex_2026-11-26/quickswap_v2_base.json")
    if base and quickswap_v2.exists():
        q2 = read(quickswap_v2)
        out.extend(["", f"QuickSwap V2 · на контрольном блоке фабрика вернула {q2['official_count']} пар. "
                    f"Все {len(q2['directory'])} пулов первой страницы подтвердили getPair и единый байткод пары. "
                    "Исходник вычитает 3 из 1000 от входа в обе стороны. В трёх парах Router02 исполнил "
                    "12 обменов с полным списанием и точной выплатой по формуле V2. У каждого повышение "
                    "минимума на единицу привело к отказу. Использован тот же метод "
                    "swapExactTokensForTokensSupportingFeeOnTransferTokens, который формирует адаптер. "
                    f"[Фабрика, байткод, обе стороны и балансы](../{quickswap_v2.as_posix()})."])
    quickswap = Path("tests/data/first_run_dex_2026-11-26/quickswap_base.json")
    if base and quickswap.exists():
        qs = read(quickswap)
        out.extend(["", f"QuickSwap V4 Algebra · у действующей фабрики `{qs['addresses']['AlgebraFactory']}` "
                    f"defaultPluginFactory указал на `{qs['active_plugin_factory']}`. У всех трёх пулов реестр "
                    "pluginByPool вернул фактический плагин. Перечисленная в документации PluginFactory "
                    f"`{qs['addresses']['PluginFactory']}` вернула другие адреса. Прокси каждого плагина сверен "
                    "по байткоду с immutable pool, слоту EIP-1967 и реализации beacon. Профиль проверяет "
                    "эти привязки по одному каноническому хешу при чтении и перед отправкой. "
                    "Между последней проверкой и включением в блок владелец beacon может изменить реализацию. "
                    "В транзакции V5 действует минимум выручки. У USDC/USDT, WETH/USDC и USDC/cbBTC "
                    "прочитаны комиссии 8, 750 и 705 миллионных. У QuoterV2 поле fee вернуло 500 во всех "
                    "случаях. В 12 из 12 симуляций V5 выход обоих квотеров совпал с приращением баланса, "
                    "точное списание входа подтверждено, повышение minOut на единицу привело к отказу. "
                    "Два внутренних обмена плагина прошли с комиссией 1. Событие Swap инициирует чтение "
                    "конечного состояния блока. Для захвата целых слов тиков в выборке потребовалось "
                    "22, 2 и 1 шаг соответственно. Владелец задаёт общий dex.integral_tick_steps для нужной глубины. "
                    f"[Реестр, размещения, обе котировки, балансы и события](../{quickswap.as_posix()})."])
    hydrex = "tests/data/first_run_dex_2026-11-26/hydrex_base.json"
    if base and Path(hydrex).exists():
        out.extend(["", "Hydrex Integral · фабрика подтвердила все 20 пулов из выборки GeckoTerminal через poolByPair. "
                    "Для WETH/USDC, USDC/cbBTC и WETH/cbBTC измерены 12 обменов V5 и 12 защитных отказов при "
                    "повышении минимума на единицу. QuoterV2, VirtualQuoter и изменение баланса владельца совпали. "
                    "Поле fee официального QuoterV2 во всех случаях равно 500. На этих пулах пользователь заплатил "
                    "соответственно 375, 145 и 124 миллионных доли. У шести операций плагин затем провёл собственный "
                    "обмен по тарифу 1. Вид algebra_integral_hydrex по событию Swap перечитывает конечное состояние "
                    "блока и комиссию следующей сделки из fee(). Двух, трёх и пяти инициализированных тиков в каждую "
                    "сторону соответственно хватило для измеренных объёмов. Общую настройку глубины "
                    "dex.integral_tick_steps для рабочего списка пулов задаёт владелец. "
                    f"[Фабрика, квотеры, карты тиков и исполнение](../{hydrex})."])
        sequential = "tests/data/first_run_dex_2026-11-26/hydrex_sequential_base.json"
        if Path(sequential).exists():
            out.extend(["", "Hydrex WETH/USDC · в четырёх симуляциях две равные доли прошли через один пул "
                        "в одном execute. Между долями плагин совершил собственный обмен. При двух входах по 1 USDC "
                        "держатель получил 591 086 786 282 727 и 591 834 767 451 764 wei WETH. Одиночные проходы "
                        "и сплиты с разными пулами доступны. Общий пул Hydrex между долями исключается до расчёта "
                        "маршрута в graph_seq::unsupported по признаку pool_after::appliable. "
                        f"[Доли, промежуточные состояния и суммы](../{sequential})."])
    saddle = "tests/data/first_run_dex_2026-11-26/saddle_ethereum.json"
    if not base and Path(saddle).exists():
        out.extend(["", "Saddle Finance · обследованы все 33 пула официального PoolRegistry на Ethereum. "
                    "У 28 пулов paused() вернул true и swap в обе стороны завершился Pausable: paused. "
                    "У EuroBP и исключённого из реестра FRAXBP-alUSD нулевые балансы. "
                    "bUSD Metapool и baoUSD-FRAXBP используют реализацию PermissionlessMetaSwap. "
                    "Профиль включает один PermissionlessSwap для BUSD, USDC и USDT. В реестре он помечен "
                    "isSaddleApproved=false. Код реализации и двух связанных библиотек совпал с артефактами Saddle. "
                    "Двенадцать симуляций от реальных держателей покрыли шесть направлений и два размера. "
                    "В каждом случае calculateSwap совпал с возвратом swap и зачисленной суммой, повышение "
                    "минимума на единицу вызвало откат. На контрольном блоке резервы составляли лишь "
                    "6,180002642489249644 BUSD, 4,508036 USDC и 4,333058 USDT. "
                    f"[Реестр, исходники и обмены](../{saddle})."])
    wombat = f"tests/data/first_run_dex_2026-11-26/wombat_{network}.json"
    if Path(wombat).exists():
        out.extend(["", "Wombat · " + ("из двух опубликованных пулов Base в профиль включён USDS/USDbC. "
                    "Две симуляции USDS → USDbC прошли. В обратную сторону обе суммы отклонил контракт "
                    "по WOMBAT_COV_RATIO_LIMIT_EXCEEDED. " if base else
                    "из семи опубликованных пулов Ethereum в профиле пять. agEUR и wstETH исполнили обе стороны, "
                    "frxETH, ETHx и USDV — проверенные направления. Два размера по каждой направленной паре "
                    "дали 16 исполнений swap и 16 контрактных отказов в шести пулах V2. У FRAX все четыре "
                    "сценария вернули CORE_UNDERFLOW. ") + "Сверены класс реализации, слот EIP-1967, "
                    "хранилище паузы каждого актива и исходники математики. Cross Chain Pool требует отдельного "
                    "адаптера версии CoreV3. "
                    f"[Реестр, исходники, состояние и обмены](../{wombat})."])
    woo = "tests/data/first_run_dex_2026-11-26/woofi_base.json"
    if base and Path(woo).exists():
        out.extend(["", "WOOFi Swap · IntegrationHelper вернул WETH и cbBTC со встречной USDC. WooPP "
                    "активен, обе цены его фактического оракула пригодны. "
                    "Слот EIP-1967 подтвердил закреплённую реализацию. В восьми исторических симуляциях "
                    "ArbExecutor V5 (шаг WOO) сумма query совпала с кредитом владельца и возвратом execute, "
                    "исполнитель списал полный вход. Для каждого размера порог на единицу выше котировки "
                    "привёл к откату, точный порог прошёл. Входы симуляции составили 1 и 100 USDC в каждой "
                    "паре, 0,0001 и 0,01 WETH, 0,000001 и 0,0001 cbBTC. Это измеренные объёмы. "
                    f"[Proxy, оракул, суммы и результаты](../{woo}) · "
                    "[Контракты WOOFi Base](https://learn.woo.org/woofi-docs/woofi-dev-docs/references/readme/base).",
                    "", "Для BSC справочник OWN_NODE_SWITCH фиксирует незаданную dex.pmm_ladder_usd и пример "
                    "положительных ступеней 50, 200, 1000, 5000 USDT. Числа рабочего профиля WOOFi Arbitrum "
                    "в справочнике размещений отсутствуют. На Base оператор задаёт dex.pmm_ladder_usd как "
                    "возрастающие суммы котируемой валюты для опроса; верхняя ступень определяет максимальный "
                    "объём, для которого есть полная котировка по лесенке. При venue.quote_asset = USDC и "
                    "dex.settlement_contract = 0x833589fcd6edb6e08f4c7c32d4f71b54bda02913 ступени выражены "
                    "в USDC: загрузчик масштабирует их по decimals = 6. Для обеих пар WooPP адрес совпадает, "
                    "поэтому dex.pmm_ladder_counter можно оставить незаданной. При другой расчётной монете "
                    "получение ступеней требует dex.pmm_ladder_counter = да и доступного курса из реестра. "
                    "По правилу первого запуска BSC/Arbitrum это переключение делается после замера нагрузки; "
                    "при N ступенях WooFi запрашивает до 2N котировок на пару за блок. Денежный результат "
                    "в USDT требует отдельного пересчёта фактической суммы USDC. "
                    "[BSC](OWN_NODE_SWITCH.md) · [Правило первого запуска](FIRST_RUN.md) · "
                    "[Размещение Arbitrum](notes/arbitrum/dex-settings-arbitrum.md)."])
    dodo_file = f"tests/data/first_run_dex_2026-11-26/dodo_{network}.json"
    if Path(dodo_file).exists():
        data_dodo = read(Path(dodo_file))
        counts = [sum(p["status"] == "quoted" for p in data_dodo["families"][kind]["pools"]) for kind in ("DVM","DPP","DSP")]
        out.extend(["", f"DODO V2 · getDODOPoolBidirection и токены пула подтвердили {counts[0]} пула DVM, "
                    f"{counts[1]} DPP и {counts[2]} DSP в выборке с положительными котировками. Каждый из них "
                    "дал выход querySellBase и querySellQuote на двух объёмах. Комиссии LP и MT приведены "
                    "для нулевого адреса, который использует загрузчик PMM. Фабрики позволяют находить пулы "
                    f"при включении поиска по фабрике. [Реестры, состояние PMM и котировки](../{dodo_file})."])
    lbfile = f"tests/data/first_run_dex_2026-11-26/liquidity_book_{network}.json"
    if Path(lbfile).exists():
        lb = read(Path(lbfile))
        count = sum(p["status"]=="quoted" for p in lb["rows"])
        out.extend(["", f"Liquidity Book · фабрика сообщает {lb['pairs']} зарегистрированных пар. Для {count} "
                    "проверены обе стороны на двух размерах с нулевым остатком входа getSwapOut. "
                    + ("В Base Swapline использует V2.2. Проверены собственные реализации и нулевые флаги расширений. "
                       if base else "У Ethereum ID индексатора с названием Trader Joe ведёт к размещению V2.1. "
                       "Адрес реализации закреплён по коду клона. В его исходнике есть отдельное изменение "
                       "удаления бина при burn. Код swap, getSwapOut и поиска следующего бина сверен с релизом V2.1. ")
                    + f"[Реестр, исходники и результаты](../{lbfile})."])
    smardex = f"tests/data/first_run_dex_2026-11-26/smardex_{network}.json"
    if Path(smardex).exists():
        info = read(Path(smardex))
        included = sum(row["own"] and row["fees"] for row in info["lineage"])
        out.extend(["", f"SmarDex · фабрика перечислила {len(info['lineage'])} пар. Свои фабрику и ABI getPairFees "
                    f"подтвердили {included}. У трёх пар записаны getAmountOutFromPair по двум сторонам и размерам. "
                    f"На одинаковых параметрах чистая функция роутера вернула {info['equal_price_example']['result'][0]} "
                    "минимальных единиц. Проверка учитывает округление при равных отношениях цен и версию библиотеки. "
                    f"[Фабрика, исходник, пары и котировки](../{smardex})."])
    if base:
        izi_file = "tests/data/first_run_dex_2026-11-26/iziswap_base.json"
        if Path(izi_file).exists():
            izi = read(Path(izi_file))
            quoted = sum(q["matched"] for p in izi["pools"] for q in p["quotes"])
            out.extend(["", f"iZiSwap · {quoted} сверок на трёх парах. Официальный quoter.swapAmount "
                        "и VirtualQuoter бота вернули одинаковый выход с использованием всего запрошенного входа. "
                        "Уровни 500 и 3000 проверены по реальным пулам дополнительно к четырём уровням конструктора. "
                        "Лестницу dex.pmm_ladder_usd задаёт оператор. "
                        f"[Фабрика, модули, уровни и оба квотера](../{izi_file})."])
        out.extend(["", "Aerodrome Slipstream · 3 разных пула WETH/USDC, по одному у каждой фабрики. "
                    "16 операций ArbExecutor.execute в eth_simulateV1, два направления и два размера у каждой фабрики, "
                    "ещё один размер у второго и третьего поколений. Размер до 0,01 WETH или 10 USDC. "
                    "Адреса источника и получателя, calldata и балансы до и после — в записи. "
                    "[Данные исполнения](../tests/data/base_slipstream_2026-11-26/executor.json) · "
                    "[Квотеры и тики](../tests/data/base_slipstream_2026-11-26/quotes.json)."])
    out.extend(["", f"Код каждого адреса проверен в [снимке размещений](../tests/data/first_run_dex_2026-11-26/{network}.json). "
                "Две записи вызова Router.getAmountOut содержат исходные аргументы и ответ. "
                "У Maverick V2 параметр dex.tick_radius задаёт оператор по глубине рынка. "
                "Значение dex.pool_enabled, доверенный исполнитель кошелька и допуск его классов задаются отдельно.",
                "", "#### GeckoTerminal", "",
                "| ID | Название | Страница пулов (HTTP, число строк) | Выбранный пул | Фабрика по пулу / обратный запрос | Статус |", "|---|---|---|---|---|---|"])
    for id, name in gt["rows"].items():
        sample = refreshed.get(id, data["sample-pools"][id])
        probe = data["factory-probes"]["rows"].get(id, {})
        url = sample["url"]
        pool = probe.get("pool") or next((a["address"] for a in sample.get("samples", []) if a.get("address")), None)
        factory = probe.get("reported_factory")
        roundtrip = probe.get("factory_roundtrip")
        if base and id == "quickswap-v2-base" and quickswap_v2.exists() and pool and any(m["pool"] == pool.lower() for m in q2["directory"]):
            factory, roundtrip = q2["factory"], "getPair(address,address)"
        if base and id == "quickswap-v4-base" and quickswap.exists() and pool and any(m["pool"] == pool.lower() for m in qs["markets"]):
            factory, roundtrip = qs["addresses"]["AlgebraFactory"], "poolByPair(address,address)"
        if base and id == "hydrex-integral" and Path(hydrex).exists():
            hx = read(Path(hydrex))
            if any(r["pool"] == pool.lower() and r["listed"] for r in hx["directory"]):
                factory, roundtrip = hx["addresses"]["Factory"], "poolByPair(address,address)"
        if id in configured:
            status = "профиль, см. параметры выше"
        else:
            task = f"T-DEX-{network}-{id}"
            if probe.get("factory_roundtrip"):
                need = "проверить код, комиссию, обе стороны и исполнение через фабрику"
            elif pool:
                need = "установить реализацию пула, привязку к протоколу и способ исполнения"
            elif sample.get("state") == "enumerated":
                need = "найти активный пул по развёртыванию и событиям сети"
            else:
                need = "восстановить перечень пулов и проверить независимый источник"
            status = f"`{task}` · {need}"
        if id == ("solidly-v3-base" if base else "solidly-v3") and solidly.exists():
            status = "DEX-19 · проверены исходники, комиссия и прямые обмены. Для торговли требуется проверенное размещение модуля"
        if sample.get("state") != "enumerated":
            status += ", страница не прочитана"
        elif not sample.get("rows_page1"):
            status += ", пустая страница снимка"
        out.append(f"| `{id}` | {cell(name)} | [{sample.get('status')}, {sample.get('rows_page1', 'нет данных')}]({url}) "
                   f"| {addresses([pool] if pool else [])} | {addresses([factory] if factory else [])}, "
                   f"{roundtrip or 'сверка не подтверждена'} | {status} |")
    out.extend(["", "#### DefiLlama", "",
                "Адреса извлечены из секции нужной сети в коде адаптера объёмов. Их роль, реализация и исполнение требуют проверки. "
                "Для строк с несколькими версиями или общей базовой ликвидностью соответствие торговому профилю устанавливается по контракту.",
                "", f"Приоритет — опубликованный оборот этой сети за предыдущие 24 часа в USD, "
                f"срез {volume.get('at', 'не получен')}. Карты, агрегаторы и общую ликвидность разных "
                "протоколов необходимо сопоставлять по контрактам. Оборот не является доходом бота.",
                "", "| ID | Площадка | 24 ч, USD | Адаптер объёмов | Адреса-кандидаты | Задача привязки |", "|---|---|---:|---|---|---|"])
    adapters = {r["id"]: r for r in data["llama-adapters"]["rows"]}
    volumes = volume.get("by_id", {})
    for row in sorted(llama["rows"], key=lambda row: -volumes.get(row["id"], -1)):
        r = adapters[row["id"]]
        ref = r.get("link") or r.get("source")
        candidates = r.get("addresses_from_chain_stanza", [])
        task = f"`T-SOURCE-{network}-{row['id']}`"
        if base and str(row["id"]) == "306" and quickswap_v2.exists() and q2["factory"] in {a.lower() for a in candidates}:
            task = "QuickSwap V2 · фабрика из адаптера подтвердила все 20 пулов первой страницы"
        if base and str(row["id"]) == "6576" and quickswap.exists() and qs["addresses"]["AlgebraFactory"] in {a.lower() for a in candidates}:
            task = "QuickSwap V4 · AlgebraFactory из адаптера подтвердила три пула и текущую фабрику плагинов"
        if str(row["id"]) == "3481" and solidly.exists() and sl["factory"] in {a.lower() for a in candidates}:
            task = "Solidly V3 · фабрика из адаптера объёмов подтвердила три пула снимка"
        if base and str(row["id"]) == "6365" and Path(hydrex).exists():
            hx_factory = read(Path(hydrex))["addresses"]["Factory"]
            if hx_factory in {a.lower() for a in candidates}:
                task = "Hydrex Integral · фабрика из адаптера подтвердила 20 пар"
        if not base and str(row["id"]) == "202" and Path(saddle).exists():
            master = read(Path(saddle)).get("master_registry")
            if master in {a.lower() for a in candidates}:
                task = "Saddle · MasterRegistry из адаптера указал на проверенный реестр 33 пулов"
        if not base and str(row["id"]) == "7599" and Path(lista).exists():
            if read(Path(lista))["factory"] in {a.lower() for a in candidates}:
                task = "Три пула ListaV3 подтверждены. Проверка пулов StableSwap из кредитных рынков ведётся отдельно"
                if Path(lista_stable).exists():
                    task = "11 уникальных пулов на 22 рынках Lista: 3 активных V3, 8 StableSwap на паузе"
        if not base and str(row["id"]) == "7979" and fermi.exists():
            candidates = [p["router"] for p in data_fermi["historical"]]
            task = "FERMI-01 · проверены два строителя, шлюз исполнения в разработке"
        out.append(f"| {row['id']} | {cell(row['name'])} | {volumes.get(row['id'], 'нет данных')} | [{cell(row['module'])}]({ref}) "
                   f"| {addresses(candidates)} | {task} |")
    out.extend(["", "#### Конфигурации Velora", "", "Проверены обозначения сети в исходном файле адаптера маршрутизации. "
                "Тип каждой реализации и возможность прямого спотового исполнения требуют проверки.",
                "", "| Адаптер | Ключ | Сетевой раздел | Адреса-кандидаты |", "|---|---|---|---|"])
    for row in va["rows"]:
        out.append(f"| {cell(row['adapter'])} | {cell(row['config_key'])} | [строка {row['line']}]({row['url']}) "
                   f"| {addresses(row['addresses'])} |")
    out.extend(["", "#### Контракты из манифестов и проверок протоколов", "",
                "Запись runtime подтверждает наличие кода на указанном блоке. Для адресов вне профилей остаётся открытой "
                "привязка ABI к конкретному сценарию исполнения. Адреса ERC-20 и отдельных пулов приведены как контрольные.",
                "", "| Назначение | Адрес | Байт runtime | Keccak-256 |", "|---|---|---|---|"])
    for r in data[evidence]["contracts"] + block["runtime"]:
        out.append(f"| {cell(r.get('contract') or r.get('name') or r.get('label'))} | `{r['address']}` "
                   f"| {r.get('bytes', r.get('runtime_bytes'))} | `{r.get('keccak256') or r.get('runtime_keccak')}` |")
    out.extend(["", "Исходные JSON исследования, SHA-256", ""])
    for n in names:
        digest = hashlib.sha256((p / (n + ".json")).read_bytes()).hexdigest()
        out.append(f"- `{n}.json` · `{digest}`")
    return out


def main():
    """Заменяет только секцию DEX, сохраняя соседние ручные разделы."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("snapshots", type=Path)
    args = parser.parse_args()
    profiles = read(Path("config/first_run_dex.json"))
    doc = Path("docs/BASE_ETHEREUM_SETTINGS.md")
    old = doc.read_text(encoding="utf8")
    start = "<!-- DEX -->"
    end = "<!-- /DEX -->"
    front, tail = old.split(start, 1)
    if end in tail:
        _, tail = tail.split(end, 1)
    lines = ["## Протоколы и размещения", "",
             "Предзаполнение DEX использует chainId, dex.chain и суффикс активной площадки. Для пустой конфигурации сначала "
             "сохраните параметры сети. Значения ниже применяются обычным сохранением полей. Оператор задаёт лимиты, "
             "глубину, кошельки и разрешение торговли по своей политике. В списке дочерних фабрик {суффикс} равен @base или @ethereum при нескольких сетях и недефолтной сети ключа. Для единственной или дефолтной сети суффикс пустой. Приёмка каждой версии исполнителя привязана к сети.",
             "", "Статус «профиль» означает наличие адресных параметров семейства в этом выпуске. В таблицах нет подтверждённых "
             "фактических сделок. Каждый ID без установленного профиля остаётся в списке работ по подключению.", ""]
    for network in ["base", "ethereum"]:
        lines.extend(tables(network, args.snapshots, profiles))
        lines.append("")
    doc.write_text(front + start + "\n\n" + "\n".join(lines) + "\n" + end + tail, encoding="utf8", newline="\n")


if __name__ == "__main__":
    main()
