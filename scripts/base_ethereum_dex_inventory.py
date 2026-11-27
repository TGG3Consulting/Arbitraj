"""Восстанавливает раздел DEX общего справочника из снимков исследования 26.11.2026 и профилей сборки.

Запуск из корня репозитория: python scripts/base_ethereum_dex_inventory.py ../outputs
Использует только файлы. Другие разделы документа сохраняются.
"""
import argparse
import hashlib
import gzip
import json
from pathlib import Path
from base_eth_omni_mars_deli import paragraphs as omni_mars_deli
import base_launch_inventory


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
    open_index=Path("tests/data/first_run_dex_2026-11-26/ethereum_open_index.json")
    open_pages=read(open_index)["pages"] if not base and open_index.exists() else {}
    refreshed={**refreshed,**open_pages}
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
    aliases = {p["settings"].get("dex.indexer_id",id):id for id,p in configured.items()}
    if len(aliases)!=len(configured):raise ValueError("Две области настроек присвоили один ID индексатора "+network)
    matched = set(gt["rows"]).intersection(aliases)
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
    out.extend(omni_mars_deli(network))
    out.extend(base_launch_inventory.recovered(network))
    if base:
        out.extend(base_launch_inventory.section())
        out.extend(base_launch_inventory.spectra())
        out.extend(base_launch_inventory.umia())
        out.extend(base_launch_inventory.vecta())
    out.extend(base_launch_inventory.fynd(network))
    out.extend(["", "#### Ручные цены источников", "", "На этой площадке `zeroex.base_url = https://api.0x.org` включает ручную оценку маршрута. Ключ `zeroex.api_key` выдаётся в [кабинете 0x](https://dashboard.0x.org/). Доли RFQ и остальных поставщиков отображаются по шагам цены. Для согласованного обмена требуется отдельная действующая фирменная котировка для своего плательщика."])
    if network=="base":
        out.extend(["", "Для книги Kipseli задайте `kipseli.base_url = https://propamm.kipseli.win` и ключ владельца `kipseli.api_key`. Для индивидуальной on-chain цены нужен зарегистрированный signer. Коммерческий доступ к подписи обмена предоставляет Kipseli. [Условия доступа](https://docs.kipseli.capital/) · [Касса и использованная подпись](../tests/data/first_run_dex_2026-11-26/kipseli_base_settled.json.gz)."])
    cfg=read(Path("tests/data/first_run_dex_2026-11-26/keyed_quote_endpoints.json"))[network]["roles"]
    targets={"zeroex.firm_target":[("allowance_holder","address"),("allowance_holder","runtime")],
             "zeroex.settler_registry":[("settler_registry",k) for k in ("address","runtime","implementation","implementation_hash")]}
    if network=="base":
        targets["kipseli.firm_target"]=[("kipseli_router","address"),("kipseli_router","runtime")]
        targets["kipseli.firm_path"]=[(role,k) for role in ("source","forward","cash") for k in ("address","runtime")]
    for key,parts in targets.items():
        out.append(f"- `{key}` = `{' '.join(cfg[role][k] for role,k in parts)}`.")
    out.extend(["", "В индивидуальном запросе владелец указывает выбранный кошелёк и свой минимальный чистый выход; для Kipseli также согласованную ставку в десятых долях базисного пункта. Бот проверяет полный долг входа и чистую выплату в дочернем блоке, с действующими балансами и разрешениями. Для 0x сверяются AllowanceHolder, Settler, две валюты, получатель и минимум в calldata. Смешанный маршрут отображает каждую ногу по ответу API. [Исходные состоявшиеся вызовы](../tests/data/first_run_dex_2026-11-26/kipseli_zeroex_settled_traces.json.gz)."])
    out.extend(["", "Пустой URL скрывает цену в кошельке выбранной площадки. Официальный секрет зашифрован, запрос собственного HTTPS-сервера отправляется без него."])
    out.extend(base_launch_inventory.spoke(network))
    if base and "umbrae-dlmm" in configured:
        ud=read(Path("tests/data/first_run_dex_2026-11-26/umbrae_base_directory.json"))
        out.extend(["","#### Umbrae на Base","",f"На высоте {ud['height']} фабрика DLMM зарегистрировала {len(ud['markets']['dlmm'])} пар, отдельная фабрика DAMM — {len(ud['markets']['damm'])}. Различаются исходники реализации, события, учёт платы и ABI обмена. [Реестры и реализации](../tests/data/first_run_dex_2026-11-26/umbrae_base_directory.json)."])
        for family in ("dlmm","damm"):
            fixture=f"tests/data/first_run_dex_2026-11-26/umbrae_{family}_base_wallets.json.gz"
            with gzip.open(fixture,"rt",encoding="utf8") as stream: settled=json.load(stream)
            cases=[c for m in settled["examples"] for c in m.get("cases",[])+m.get("additional",[])]
            rounds=sum(any(c.get("sell") and "net" in c["sell"] for c in m.get("cases",[])+m.get("additional",[])) for m in settled["examples"])
            outcomes=[]
            for mode in ("native","module","v5"):
                legs=[l for c in cases if c["mode"]==mode for l in (c["buy"],c["sell"]) if l and "net" in l]
                outcomes.append(f"{mode} — {len(legs)} выполненных ног, {min(l['gas'] for l in legs)}–{max(l['gas'] for l in legs)} газа")
            out.append(f"**{family.upper()}.** Круговой обмен подтверждён на {rounds}/{len(settled['examples'])} рынках для проверенных объёмов. "+"; ".join(outcomes)+f". [Вызовы, отказы и балансы](../{fixture}).")
        out.append("В DLMM штатный роутер проверяет поступление получателю и deadline. Положительный ответ viewer принимается только при полном расходе входа и завершённом проходе по бинам. На рынке `0x39460a75…` из заявки на 100000000000000 wei WETH viewer учёл 468754580167 wei и вернул 997 минимальных единиц USDC. Обмен исходной суммы откатился. Для лестницы оператор задаёт `dex.pmm_ladder_usd` по собственному лимиту. Независимая ступень на стороне встречной валюты требует курса и `dex.pmm_ladder_counter`.")
        out.append(f"DAMM учитывает удержанную плату в активе feeToken, начисления протоколу и LP и смену ставки при следующем обмене. У обычного вызова пары нет срока и сравнения с выплатой после перевода. Для защищённого обмена в обоих семействах доступен один модуль со свёркой полного расхода кошелька и чистого выхода. Архивный runtime `{settled['module_hash']}`, оценка размещения {settled['deployment_gas']} газа. Адрес задаёт владелец после размещения через `dex.router`, выбрав `umbrae_damm_module` либо `umbrae_dlmm_module`. Модуль допускает прямой вызов и шаг 17 V5.")
    if base and "hello-dex-base" in configured:
        file="tests/data/first_run_dex_2026-11-26/hello_base_wallets.json.gz"
        with gzip.open(file,"rt",encoding="utf8") as stream: hello=json.load(stream)
        directory=read(Path("tests/data/first_run_dex_2026-11-26/hello_base_directory.json"))
        markets=directory["markets"]
        out.extend(["","#### HELLO V3 на Base","",
                    f"В реестре {len(markets)} рынков, активная ликвидность ненулевая у {sum(int(m['liquidity'])>0 for m in markets)}. "
                    f"Код создания пулов извлечён из HelloV3PoolDeployer `{directory['deployer']}`. В точке снимка lmPool() равен нулю во всех рынках. "
                    "Финансовый путь проверяет создателя, реестр в обе стороны, codehash фабрики и квотера, callback и отсутствие нового модуля наград. "
                    "Штатные Router и Router02 проверяют минимальный номинальный выход. В торговом профиле используется V5 с контролем реального прироста у получателя.",
                    "", "| Пул и входной актив | Вход → выплата → возврат в минимальных единицах | Газ покупки и продажи V5 |",
                    "|---|---:|---:|"])
        for deal in hello["settlements"]:
            for case in deal["cases"]:
                if case["mode"]=="v5":
                    a,b=case["buy"],case["sell"]
                    out.append(f"| `{deal['pool']}` / `{deal['cash']}` | {a['input']} → {a['net']} → {b['net']} | {a['gas']} / {b['gas']} |")
        out.append(f"[Реестр CREATE2](../tests/data/first_run_dex_2026-11-26/hello_base_directory.json) · [Дельты трёх способов оплаты, minOut+1 и deadline](../{file}).")

    if not base:
        market_path = Path("tests/data/first_run_dex_2026-11-26")
        with gzip.open(market_path / "safeswap_ethereum_wallets.json.gz", "rt") as stream:
            safeswap = json.load(stream)
        with gzip.open(market_path / "surge_ethereum_wallets.json.gz", "rt") as stream:
            surge = json.load(stream)
        out.extend(["", "#### SafeSwap и Surge на Ethereum", "",
                    "У SafeSwap четыре фабрики. Реестры сверены по CREATE2 и getPair в обоих порядках на одном блоке. На трёх фабриках сравнивались Router02 и V5. У четвёртой проверен вход через общедоступный SafeSwapTradeRouter с отдельной оплатой в ETH.",
                    "", "| Фабрика SafeSwap | Зарегистрировано пар | Пар с полным кругом | Выплаченных ног V5 / Router02 / SafeSwapTradeRouter |",
                    "|---|---:|---:|---:|"])
        for venue in safeswap["deployments"]:
            markets = venue["markets"]
            legs = {mode: sum(1 for m in markets for r in m.get("rounds", []) if r["mode"]==mode
                              for leg in ("buy", "sell") if r.get(leg, {}).get("exact", {}).get("status")=="0x1")
                    for mode in ("v5", "router")}
            def paid(row, side):
                return row.get(side, {}).get("exact", {}).get("status") == "0x1"
            settled = sum(any(paid(r, "buy") and paid(r, "sell")
                              for r in m.get("rounds", []) + m.get("paid_public", [])) for m in markets)
            public = sum(paid(r, side) for m in markets for r in m.get("paid_public", []) for side in ("buy","sell"))
            count = f"{legs['v5']} / {legs['router']} / {public}"
            out.append(f"| {venue['id']} · `{venue['roles']['factory']['address']}` | {len(markets)} | {settled} | {count} |")
        with gzip.open(market_path / "safeswap_ethereum_protected_wallets.json.gz", "rt") as stream:
            protected_safe = json.load(stream)
        cases = [r for r in protected_safe["cases"] if r["profile"]=="proxy-2"]
        out.extend(["", "Оплата SafeSwap Trade из WETH и отдельным платежом ETH на одном и том же объёме покупки.",
                    "", "| Путь | Списано WETH, raw | Плата покупки, wei ETH сверх WETH | Получено LINK, raw | Получено обратно WETH, raw | Плата продажи, wei ETH сверх WETH | Газ покупки / продажи |",
                    "|---|---:|---:|---:|---:|---:|---:|"])
        safe_modes = {"module":"Модуль, плата из WETH", "v5":"V5 через модуль, плата из WETH", "separate":"Модуль, плата в ETH"}
        for row in cases:
            b, s = (row[side]["exact"] for side in ("buy", "sell"))
            out.append(f"| {safe_modes[row['mode']]} | {b['spent']} | {b['external_fee']} | {b['received']} | {s['received']} | {s['external_fee']} | {b['gas']} / {s['gas']} |")
        out.extend(["", "При проверке двух размеров покупки каждым из трёх способов модуль сохранил начальный 1 wei в публичном роутере и выплатил столько же LINK. Все шесть обратных заявок при таком остатке получили отказ до расхода средств. Для автоматического пути кошельку требуется собственный модуль с совпадающими ролями. При продаже за WETH резерв платы модуля возвращается из выручки в том же обмене. [Вызовы, балансы и газ](../tests/data/first_run_dex_2026-11-26/safeswap_ethereum_protected_wallets.json.gz)."])
        for market in safeswap["deployments"][3]["markets"]:
            for row in market.get("paid_public", []):
                if paid(row,"buy") and paid(row,"sell"):
                    b, s = (row[side]["exact"] for side in ("buy","sell"))
                    out.append(f"\nПубличный роутер proxy-2, пара `{market['pool']}`: {b['spent']} raw WETH → {b['received']} raw LINK → {s['received']} raw WETH. Комиссия покупки и продажи составила {b['fee']} и {s['fee']} wei ETH, помимо газа.")
        out.extend(["", "У исходной пары ставка 0,20%, у её Router02 расчёт выплаты использует 0,25%. У второй фабрики ставка обоих путей 0,20%. Для каждого размера сравниваются выплата и газ. [Реестр, вызовы и балансы](../tests/data/first_run_dex_2026-11-26/safeswap_ethereum_wallets.json.gz).",
                    "", f"У Surge проверен рынок ETH/SRG по адресу `{surge['address']}` на блоке {surge['height']}. "
                    "Покупка принимает ETH, продажа выплачивает ETH. Продажа тем же адресом в блоке его покупки откатывается. Минимум на продаже применяется к валовой выплате до удержания комиссии.",
                    "", "| Покупка, wei ETH | Получено, raw SRG | Продажа, wei ETH net | Газ покупки / продажи |", "|---:|---:|---:|---:|"])
        for r in surge["cases"]:
            out.append(f"| {r['input']} | {r['bought']} | {r['received']} | {r['buy']['gas']} / {r['sell']['gas']} |")
        out.append("Газ оплачивается отдельно. Валовая и чистая выплаты совпали при ставке архивного блока. [Квитанции прямого ETH/SRG](../tests/data/first_run_dex_2026-11-26/surge_ethereum_wallets.json.gz).")
        with gzip.open(market_path / "surge_ethereum_protected_wallets.json.gz", "rt") as stream:
            guarded = json.load(stream)
        out.extend(["", "| Исполнение | Валюта | Бюджет raw | Куплено SRG raw | Обратно raw | Газ покупки / продажи | Газ разрешений |", "|---|---|---:|---:|---:|---:|---:|"])
        for case in guarded["cases"]:
            currency = "ETH" if case.get("native_cash") else "WETH" if case["cash"] == guarded["wrapped"] else "USDT"
            buy, sell = case["buy"], case["sell"]
            out.append(f"| {case['mode']} | {currency} | {buy['spent']} | {buy['received']} | {sell['received']} | {buy['gas']} / {sell['gas']} | {' / '.join(map(str,case['approval_gas']))} |")
        out.append("Для SRG покупки и продажи идут в последовательных блоках. В каждом направлении архив проверяет остатки плательщика, минимум на единицу выше выплаты и истечение срока. Прямые формы с ETH и WETH и шаг V5 требуют проверенного адреса развёрнутого модуля в настройках владельца. [Симуляции обеих форм и маршрута из USDT](../tests/data/first_run_dex_2026-11-26/surge_ethereum_protected_wallets.json.gz).")

    if not base and "toonswap" in configured:
        toon_path = Path("tests/data/first_run_dex_2026-11-26/toonswap_ethereum_wallets.json.gz")
        with gzip.open(toon_path, "rt", encoding="utf8") as stream:
            toon=json.load(stream)
        out.extend(["", "#### ToonSwap на Ethereum", "",
                    f"Фабрика зарегистрировала {len(toon['markets'])} пар. На блоке {toon['height']} получено {len(toon['cases'])} выплат, "
                    f"штатным Router {sum(c['wire']=='router' for c in toon['cases'])} и V5 {sum(c['wire']=='v5' for c in toon['cases'])}. "
                    "Комиссия обмена 0,25%. Из неё пара отдельными переводами выплачивает долю рефералу и казначейству. "
                    f"По этому блоку доли составляют {toon['rates'][0]}/10000 и {toon['rates'][1]}/10000. "
                    f"В {sum(c['reserve_model_matches'] for c in toon['cases'])} случаях последующий резерв подтверждён событием Sync. "
                    f"В архиве также {len(toon['unavailable'])} отказов. [Оплаты, суммы и отказы](../{toon_path.as_posix()})."])
    if not base and "sashimiswap" in configured:
        out.extend(["", "#### SashimiSwap на Ethereum", "",
                    "На блоке 26494988 фабрика зарегистрировала 83 пары. Внутренний учёт Router02 совпал с обоими резервами у 80 ненулевых пар. "
                    "Остатки ERC20 на самих 83 парах нулевые. При обмене платит Router02 с проверкой фактического прироста баланса получателя. "
                    "При нехватке остатка роутер запрашивает средства у общего хранилища. Возможность выплаты на выбранный объём проверяет кошельковая симуляция.",
                    "На 53 парах проверены 102 входа. Кошельковой симуляцией подтверждены 73 покупки и 56 обратных продаж, включая полные циклы на 31 паре. "
                    "У 16 покупок вырос показанный токеном баланс, но его Transfer в квитанции отсутствовал, а чтение allowance перед обратной продажей откатилось. "
                    "У десяти таких контрактов совпал весь runtime (608 байт). [Привязка каждой покупки к коду](../tests/data/first_run_dex_2026-11-26/sashimi_ethereum_token_evidence.json). "
                    "[Архив GitHub](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45925784274).",
                    "[Фабрика в SDK](https://github.com/SashimiProject/sashimiswap-sdk/blob/50fb3f0b7fcc7108be957a0616158a952f60ca2e/src/constants.ts) · "
                    "[Реестр, остатки и хранилище](../tests/data/first_run_dex_2026-11-26/sashimi_ethereum_registry.json.gz)."])
    if not base:
        out.extend(["","### Bundles Swap и DAGrow на Ethereum, блок 26 494 988","",
                    "У Bundles фабрика `0xacff9eee0a5522000e7141b77107359a6462e8d2`, Router `0x7088d2a3855bf69320869ea28e5d95a142935441`, "
                    "токен комиссии BUN `0x695f775551fb0d28b64101c9507c06f334b4ba86`. Зарегистрировано 11 пар. У 10 обычных пар плата за обмен поступает в BUN. "
                    "Пара WETH/BUN `0x9dd78ea2b7a92b6cb5d4a495dac34f8641070ceb` удерживает комиссию из токена протокола. "
                    "На 11 парах подтверждены 84 обмена штатного Router, 84 прямых обмена защищённого модуля и 44 через V5. Повышенный минимум и истёкший срок вернули исходные остатки. [Проверка 128 обменов](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45931393415/job/140284613266). [Профили обычных переводов](../tests/data/first_run_dex_2026-11-26/bundles_exact_transfers.json). [Реестр, хеши кода и квитанция роутера](../tests/data/first_run_dex_2026-11-26/bundles_ethereum_directory.json.gz).",
                    "У DAGrow адрес пула `0x9fc2038b773bde9e44fdaba0e5093647f53c5105`, реализация `0x56202a54fb68fb456f1879f9add63d37e155d6bc`. "
                    "Guard `0xdde4b3cf28ad1409d6c79cf1bdc298424362e2ab`, реализация `0x0005144b17cf95d6b0f2d682abbccf5596db0a27`. "
                    "За 1 USDT пул котировал 0,876418871064609516 DAGR для Eco и 0,841008007587251556 DAGR для Exo. "
                    "Для торговли важны адрес плательщика, флаг паузы продаж и обе реализации. Резервы выражены с 18 знаками, платёж USDT имеет 6. "
                    "SSP сравнивает точный вход и точный выход, защитный модуль измеряет конечные балансы кошелька. На двух размерах и обеих сторонах прошли 20 вызовов: 8 прямых, 8 защищённых прямых, 4 через V5. Для штатного SSP возврат в изменяемый USDT требует защиты конечной выплаты. [Денежные результаты и откаты](../tests/data/first_run_dex_2026-11-26/dagrow_ethereum_protected_wallets.json.gz). "
                    "[Архив котировок с исходными единицами](../tests/data/first_run_dex_2026-11-26/dagrow_ethereum_directory.json.gz) · [Описание протокола](https://dagrow.app/whitepaper/)."])
    if not base and {"unicly","hopeswap","archly-ethereum"}.issubset(configured):
        out.extend(["", "### Unicly, HopeSwap и Archly на Ethereum", "",
            "Контроль на каноническом блоке 26494988 использовал ETH независимого кошелька в архивной симуляции. "
            "Стейблкойны для входа куплены через публичный обмен. Для каждой успешной ноги записаны расход, приход, газ и проверка отказа на 1 минимальную единицу выше выплаты.",
            "", "| Площадка | Реестр и кошельковые обмены | Выбор исполнения |", "|---|---|---|",
            "| Unicly | 91 пара в реестре. Пять пар прошли оба направления в двух размерах через Router и V5, всего 40 выплат. "
            "По всем 91 расчёт CREATE2 из развёрнутой фабрики совпадает с регистрацией, опубликованный init-хеш SDK предсказывает другой адрес. "
            "| В Router активен обычный exact-in. Функция SupportingFeeOnTransferTokens находится в комментарии проверенного исходника. "
            "Для DAI/WETH выплата и подписанное поручение восстанавливаются из реальных событий. При одинаковых разрешениях штатный Router потратил на 17 218–17 261 газа меньше V5. "
            "Другие выходные токены требуют отдельного подтверждения точного перевода для прямого Router. "
            "[Архив](../tests/data/first_run_dex_2026-11-26/unicly_ethereum_wallets.json.gz) · [SDK](https://github.com/0xLeia/unicly-sdk/blob/81a2f6c0833b8325343a432bbb309198e39b4af0/src/constants.ts) |",
            "| HopeSwap | Из четырёх зарегистрированных пар две с USDT завершили 16 выплат. Две с третьим токеном 0xf5c6…d4d8 "
            "вернули TRANSFER_FAILED при попытке его получения. Подготовка собственного остатка этого токена для обратного входа не удалась. "
            "| getFeeRateNumerator сообщает долю после комиссии. На четырёх парах фактические ставки 30, 30, 10, 30 из 10000. "
            "Штатный Router с контролем дельты получил ту же выплату при расходе на 18 001–18 321 газа меньше V5 в восьми сопоставлениях. "
            "[Архив](../tests/data/first_run_dex_2026-11-26/hopeswap_ethereum_wallets.json.gz) · [Документация HopeSwap](https://docs.hope.money/protocols/hopeswap) |",
            "| Archly | Обе пары volatile, 16 успешных выплат на ARC/WETH и ARC/USDC. На проверенной высоте котируемая сторона содержит "
            "0,000928288945292464 WETH и 2,895441 USDC соответственно. Рыночная ёмкость мала. "
            "| Фабрика считает персональную ставку getRealFee(pair). При нулевых overrides ставка обеих равна 30/10000. "
            "Штатный роутер проверяет дельту получателя, при равном разрешении его обмен использовал на 8 696–10 713 газа меньше V5. "
            "Прямую оплату разрешают после проверки точного дебета входного ERC20, на первом запуске заданы WETH и ARC. "
            "[Архив](../tests/data/first_run_dex_2026-11-26/archly_ethereum_wallets.json.gz) · [Размещения Archly](https://docs.archly.fi/security) · "
            "[Проверенный код ARC](https://sourcify.dev/server/v2/contract/1/0x9482c407d32204462d8cbbc0755e96c39b79878e) |",
            "", "У Unicly публикация второго поколения описывает аукционы NFT и выпуск их долей. "
            "Независимая вторая фабрика спотового AMM этой публикацией не подтверждается. "
            "У Archly Ethereum и Archly Base второго поколения совпадает фактический runtime фабрики. На Base проверены 428 из 428 пар, "
            "все индивидуальные overrides нулевые на блоке 62262756. Предыдущее поколение Base читает только ставку типа кривой. "
            "[Публикация команды Unicly](https://medium.com/unicly/unicly-v2-launch-airdrop-announcement-fcfd1beed6dc) · "
            "[Архив индивидуальных ставок Base](../tests/data/first_run_dex_2026-11-26/archly_base_fee_overrides.json.gz)."])
    for key,pid,name in (("blueprint","blueprint","Blueprint"),("vvs","vvs-v3-ethereum","VVS V3")):
        if base or pid not in configured:
            continue
        directory=Path(f"tests/data/first_run_dex_2026-11-26/{key}_ethereum_directory.json.gz")
        with gzip.open(directory,"rt",encoding="utf8") as stream: registry=json.load(stream)
        rows=registry["markets"]
        out.extend(["",f"#### {name} на Ethereum","",
                    f"На блоке {registry['height']} по событиям фабрики, CREATE2 и nonce создателя подтверждены все {len(rows)} пулов и {len(registry['tiers'])} уровня комиссии. "
                    f"Текущая ликвидность положительна у {sum(int(r['liquidity'])>0 for r in rows)} пулов. Достижимые позиции по обе стороны от текущего тика проверяются отдельно. "
                    f"[Адреса контрактов, транзакции регистрации и состояние](../{directory.as_posix()})."])
        if key=="blueprint":
            out.append("Официальный SwapRouter с точной передачей обоих ERC20 и шаг V3 ArbExecutor сравниваются с учётом газа и необходимых разрешений.")
        else:
            out.append("VVS применяет отдельный callback. Прямой защищённый модуль с этой фабрикой и его вызов через шаг 17 V5 доступны при установленном dex.router. "
                       "Для официального SwapRouter выставляется dex.v3_native_router. WETH и CosmosERC20 VVS поддерживают точные переводы. "
                       "Два рынка работают с VVSV3LmPool. Бот сверяет привязку пула и реализацию MasterChef через EIP-1967.")
        wallets=directory.with_name(f"{key}_ethereum_wallets.json.gz")
        if wallets.exists():
            with gzip.open(wallets,"rt",encoding="utf8") as stream: paid=json.load(stream)
            measured=paid["cases"]
            out.append(f"В архивной симуляции подтверждены {len(measured)} выплат на {len({r['pool'] for r in measured})} рынках. "
                       f"Для каждой выплаты проверены полученная сумма, газ, отказ при минимуме на единицу выше и истёкший срок. "
                       f"[Транзакции, события и балансы](../{wallets.as_posix()}).")
            if key=="vvs":
                ident={registry["exact_token"]["address"],"0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"}
                groups={}
                for r in measured:
                    if {r["source"],r["target"]}==ident:
                        groups.setdefault((r["source"],r["spent"],r["received"]),{})[r["wire"]]=r["gas"]
                delta=[r["module"]-r["router"] for r in groups.values() if {"module","router"}<=r.keys()]
                if delta:out.append(f"При одинаковом полном расходе и поступлении WETH/VVS штатный Router экономил от {min(delta)} до {max(delta)} единиц газа относительно прямого модуля. Покупка и продажа проверены на двух объёмах. Стоимость нового approve относится к конкретному плательщику.")
    for key,name in (("wagmi","WAGMI"),("lif3","Lif3")):
        if base or key+"-ethereum" not in configured:
            continue
        prefix=Path(f"tests/data/first_run_dex_2026-11-26/{key}_ethereum")
        registry_path=Path(str(prefix)+"_registry.json")
        wallet_path=Path(str(prefix)+"_wallets.json.gz")
        reg=read(registry_path)
        with gzip.open(wallet_path,"rt",encoding="utf8") as stream:
            rows=json.load(stream)["cases"]
        groups={}
        for r in rows:
            groups.setdefault((r["pool"],r["source"],r["spent"],r["received"]),{})[r["wire"]]=int(r["gas"])
        deltas=[g["v5"]-g["router"] for g in groups.values() if "v5" in g and "router" in g]
        directions=len({(r["pool"],r["source"]) for r in rows})
        exact={"0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2","0x2260fac5e5542a773aa44fbcfedf7c193bc2c599"}
        eligible=[r for r in rows if r["wire"]=="router" and {r["source"],r["target"]}==exact]
        savings=[groups[(r["pool"],r["source"],r["spent"],r["received"])]["v5"]-int(r["gas"]) for r in eligible]
        approval_gap=[int(r["approval_gas"])-saved for r,saved in zip(eligible,savings,strict=True)]
        out.extend(["",f"#### {name} V3 на Ethereum","",
                    f"Блок {reg['height']}. У фабрики {len(reg['markets'])} пулов. У собственного плательщика подтверждены {len(rows)} выплат "
                    f"в {directions} из {2*len(reg['markets'])} направлений. При одинаковых расходе и выплате V5 потребовал на {min(deltas):,}–{max(deltas):,} единиц газа больше штатного Router. Стоимость разрешения зависит от кошелька и адреса получателя allowance.",
                    f"Для WBTC/WETH получено {len(eligible)} сопоставимых расчётов. При имеющихся разрешениях экономия Router составила {min(savings):,}–{max(savings):,} единиц газа. Если разрешение есть только у V5, добавление измеренного approve Router меняет выбор: V5 дешевле на {min(approval_gap):,}–{max(approval_gap):,} единиц газа.",
                    "Перед обменом проверяются CREATE2, комиссия, оба порядка getPool и канонический блок. Прямая нога Router требует обе записи адреса и хеша в dex.router_exact_tokens данной DEX и проверенный код роутера. В этой выборке условия выполняет WBTC/WETH. Для других пар и составных маршрутов применяется доступный V5. При восстановлении прямой ноги сверяются подписанный вызов, единственный Swap этого пула и фактические чистые Transfer кошелька.",
                    f"[Адреса площадки]({reg['source']}) · [Регистрации](../{registry_path.as_posix()}) · [Квитанции и дельты](../{wallet_path.as_posix()})."])
    if not base and "concave" in configured:
        cv_path=Path("tests/data/first_run_dex_2026-11-26/concave_ethereum_wallets.json.gz")
        with gzip.open(cv_path,"rt",encoding="utf8") as stream:
            cv=json.load(stream)
        fills=cv["cases"]
        directed={(r["pool"],r["source"]) for r in fills}
        modes={}
        for r in fills:
            modes.setdefault((r["pool"],r["source"],r["spent"]),{})[r["wire"]]=r
        both=[v["v5"]["gas"]-v["router"]["gas"] for v in modes.values() if "v5" in v and "router" in v]
        out.extend(["", f"Concave Ethereum · Официальный GemswapFactory зарегистрировал {len(cv['markets'])} клонов EIP-1167 "
                    f"типа 1. У каждого сверены исходная реализация, солёный CREATE2, регистрация в обоих порядках и "
                    f"комиссия 25 базисных пунктов. Получено {len(fills)} выплат через GemswapRouter и V5 в {len(directed)} из "
                    f"{2*len(cv['markets'])} направлений. При одинаковой фактической выплате разница газа V5 минус Router "
                    f"составила от {min(both):,} до {max(both):,}, стоимость разрешения и текущий газ учитываются при выборе. "
                    f"У DAI/USDC резерв USDC всего одна минимальная единица. "
                    f"[Кошелёк, исходный блок и квитанции](../{cv_path.as_posix()}) · "
                    f"[Официальные адреса]({cv['sources'][0]})."])
    if not base and "degenswap-ethereum" in configured:
        ds=Path("tests/data/first_run_dex_2026-11-26")
        def observed(id):
            with gzip.open(ds/f"{id}_ethereum_wallets.json.gz","rt",encoding="utf8") as f:return json.load(f)
        degen=observed("degenswap")
        native=[r for case in degen["outcomes"] if case["mode"]=="router" for r in case.get("rounds",[])]
        npositive=sum(all(int(v or 0)>0 for v in p["balances"]+p["reserves"]) for p in degen["markets"])
        out.extend(["",f"DegenSwap Ethereum · Фабрика зарегистрировала {len(degen['markets'])} пар WETH, у {npositive} положительны оба резерва и оба баланса. Штатный роутер подтвердил {len(native)} покупок из {sum(case['mode']=='router' for case in degen['outcomes'])} размеров. У {sum('sell_error' in r for r in native)} обратных продаж штатная проверка расхода и минимальной выплаты не прошла. Роутер уменьшает подписанный минимум, когда комиссия взимается в выходной валюте. Для индивидуального модуля проверяются плательщик, физические токены, CREATE2, изменяемые сборы и точная дельта получателя. [Реестр и результаты по размерам](../tests/data/first_run_dex_2026-11-26/degenswap_ethereum_wallets.json.gz)."])
        with gzip.open(ds/"degenswap_ethereum_protected.json.gz","rt",encoding="utf8") as f:protected=json.load(f)
        combos=protected["cases"]
        for mode in ("module","v5"):
            rows=[r for r in combos if r["mode"]==mode]
            out.append(f"DegenSwap, {mode} · {sum('buy' in r for r in rows)} оплат покупок на {len(rows)} размеров, {sum('sell' in r for r in rows)} продаж купленного. В {sum('funding' in r.get('sell_refused','') for r in rows)} случаях ERC20 зачислил модулю меньше полного входа. [Квитанции, фактические суммы и газ](../tests/data/first_run_dex_2026-11-26/degenswap_ethereum_protected.json.gz).")
        bounded=[r for case in degen["outcomes"] if case["mode"]=="bounded-router" for r in case.get("rounds",[])]
        if bounded:
            out.append(f"Штатный Router с увеличенным подписанным минимумом подтвердил {len(bounded)} покупок и {sum('sell' in r for r in bounded)} обратных продаж из купленного. Фактическая выплата и максимально гарантируемый минимум записаны отдельно. Для прямого исполнения требуются проверенные свойства дебета входного ERC20. Для reflection-токенов модуль вычисляет дебет через balanceOf.")
        with gzip.open(ds/"degenswap_ethereum_funded.json.gz","rt",encoding="utf8") as f:funded=json.load(f)
        for mode,label in (("module","Модуль с измерением обоих переводов"),("v5","V5 через тот же модуль")):
            rows=[r for r in funded["cases"] if r["mode"]==mode]
            out.append(f"{label} оплатил {sum('buy' in r for r in rows)} покупок и {sum('sell' in r for r in rows)} продаж на {len(rows)} исходных размерах. Комиссия перевода может уменьшить зачисление модулю. Плательщик оплачивает полный фактический дебет. [Выплаты, откаты и остатки](../tests/data/first_run_dex_2026-11-26/degenswap_ethereum_funded.json.gz).")
        cypher=observed("cypher_v2")
        native=[r for case in cypher["outcomes"] if case["mode"]=="router" for r in case.get("rounds",[])]
        executor=[r for case in cypher["outcomes"] if case["mode"]=="v5" for r in case.get("rounds",[])]
        out.extend(["",f"Cypher V2 Ethereum · {len(cypher['markets'])} зарегистрированных пар. Из {len(native)} объёмов Router подтвердил {sum('sell' in r for r in native)} полных оборотов, V5 подтвердил {sum('sell' in r for r in executor)} из {len(executor)}. В остальных случаях обе формы вернули INSUFFICIENT_OUTPUT_AMOUNT на продаже. У каждой пары собственная изменяемая комиссия fee()/100000. [Реестр, квитанции и расходы](../tests/data/first_run_dex_2026-11-26/cypher_v2_ethereum_wallets.json.gz)."])
        with gzip.open(ds/"cypher_v4_ethereum_directory.json.gz","rt",encoding="utf8") as f:cl=json.load(f)
        out.extend(["",f"Cypher V4 Ethereum · Фабрика Algebra зарегистрировала {len(cl['markets'])} пулов. Обычные и пользовательские развёртывания используют разные ключи реестра и комиссии плагинов. [Адреса и конфигурации на каноническом блоке](../tests/data/first_run_dex_2026-11-26/cypher_v4_ethereum_directory.json.gz)."])
        cyp_cases=[]
        for part in range(4):
            with gzip.open(ds/f"cypher_v4_ethereum_wallets_{part}.json.gz","rt",encoding="utf8") as f:cyp_cases.extend(json.load(f)["markets"])
        extra=[]
        for part in range(4):
            with gzip.open(ds/f"cypher_v4_ethereum_extra_{part}.json.gz","rt",encoding="utf8") as f:extra.extend(json.load(f)["markets"])
        found={m["pool"] for m in extra if any("sell" in c for c in m["cases"])}
        both={m["pool"] for m in cyp_cases+extra if any("sell" in c for c in m["cases"])}
        out.append(f"Повторная проверка {len(extra)} ранее не подтверждённых пулов выявила ещё {len(found)} рынков с покупкой и продажей. В {sum(m['pool'] in found and int(m['liquidity'])==0 for m in extra)} из них начальный тик имел нулевую активную ликвидность. На проверенных размерах обе стороны оплатились в {len(both)} из {len(cl['markets'])} зарегистрированных пулов. " + " · ".join(f"[Дополнительная серия {p+1}](../tests/data/first_run_dex_2026-11-26/cypher_v4_ethereum_extra_{p}.json.gz)" for p in range(4))+".")
        levels={}
        for m in cyp_cases+extra:
            for case in m["cases"]:
                for side in ("buy","sell"):
                    if side not in case:continue
                    x=case[side]["exact"]; levels.setdefault((m["pool"],case["size"],side),{})[case["mode"]]=int(x["gas"],16)
        diffs=[v["v5"]-v["router"] for v in levels.values() if "v5" in v and "router" in v]
        out.append(f"У Cypher V4 обе формы провели по {len(diffs)} отдельных обменов, Router экономил {min(diffs):,}–{max(diffs):,} единиц газа. При выборе учитываются разрешения владельца. Оплата V5 остаётся доступна для проверенной пары с нестандартным ERC20. [Архив, часть 1](../tests/data/first_run_dex_2026-11-26/cypher_v4_ethereum_wallets_0.json.gz) · [2](../tests/data/first_run_dex_2026-11-26/cypher_v4_ethereum_wallets_1.json.gz) · [3](../tests/data/first_run_dex_2026-11-26/cypher_v4_ethereum_wallets_2.json.gz) · [4](../tests/data/first_run_dex_2026-11-26/cypher_v4_ethereum_wallets_3.json.gz).")
        baseline=observed("baseline")
        rows=[case for pool in baseline["markets"] for case in pool["cases"]]
        deals=sum(isinstance(case.get(side),dict) and case[side].get("stage")=="accepted" for case in rows for side in ("buy","sell"))
        out.extend(["",f"Baseline Ethereum · Отдельный реестр Relay и V4 PoolManager связывает {len(baseline['markets'])} bToken с физическим WETH. У Router и V5 проверены два объёма с каждой стороны, выплата, минимальный выход и истечение срока. Из {2*len(rows)} обменов подтверждено {deals}. [Оплата и поступление на Ethereum](../tests/data/first_run_dex_2026-11-26/baseline_ethereum_wallets.json.gz)."])
    if not base and "bitswap" in configured:
        bs=read(Path("tests/data/first_run_dex_2026-11-26/bitswap_ethereum_registry.json"))
        directory_path=Path("tests/data/first_run_dex_2026-11-26/bitswap_ethereum_directory_wallets.json.gz")
        with gzip.open(directory_path,"rt",encoding="utf8") as stream:
            wallets=json.load(stream)
        fills=wallets["cases"]
        pools=len({r["pool"] for r in fills})
        edges=sum(int(r["quote"][0])<int(r["authorized"]) for r in fills)
        change=sum(int(r["spent"])<int(r["quote"][0]) for r in fills)
        same=sum(any(hop[2]==r["pool"] for hop in r["funding"]) for r in fills)
        out.extend(["", f"BitSwap Ethereum · По NewPool, CREATE2 и pool(x,y,fee) проверены {len(bs['markets'])} пулов и пять "
                    f"делегированных реализаций. Кошелёк исполнил {len(fills)} обмена в {pools} пулах, в обе стороны по два размера. "
                    f"Газ шага вместе с расчётом кошелька {min(r['gas'] for r in fills):,}–{max(r['gas'] for r in fills):,}. "
                    f"В {same} опытах покупка исходного токена изменила проверяемый пул до измерения. "
                    f"В {edges} опытах достигнута граница ликвидности, ещё в {change} вернулась сдача округления. "
                    f"Каждый успешный обмен сверяет выплату, срок и откат при увеличении минимума на единицу. "
                    f"Для работы требуются собственный кошелёк с привязанным проверенным ArbExecutor V5 и котировка на текущем блоке. "
                    f"[Реестр и события](../tests/data/first_run_dex_2026-11-26/bitswap_ethereum_registry.json) · "
                    f"[Баланс, газ и отказы](../{directory_path.as_posix()})."])
        direct_path=directory_path.with_name("bitswap_ethereum_wallets.json.gz")
        with gzip.open(direct_path,"rt",encoding="utf8") as stream:
            controls=json.load(stream)["cases"]
        cuts=sum(int(r["quote_used"])<int(r["authorized"]) for r in controls)
        out.append(f"На рынках WETH/USDT и WETH/USDC выполнены ещё {len(controls)} контрольных обменов при "
                   f"входах 1, 10 и 2000 единиц стейблкойна или 0,001, 0,01 и 3 WETH. В {cuts} случаях порученный размер "
                   f"достиг края ликвидности. Такой объём исключается из исполнимой лесенки. Остаток входа возвращён кошельку. "
                   f"[Суммы и Transfer в пул](../{direct_path.as_posix()}).")
    if base:
        sobal_path = Path("tests/data/first_run_dex_2026-11-26/sobal_base_directory.json")
        if sobal_path.exists():
            directory = read(sobal_path)
            pools = directory["pools"]
            by_family = {k: sum(k in p["families"] for p in pools) for k in directory["factories"]}
            out.extend(["", f"Sobal · Vault `{directory['vault']}`. По событиям регистрации восстановлены все {len(pools)} "
                        f"пулов блока 62 262 756. Взвешенных {by_family['weighted']}, композитных стабильных "
                        f"{by_family['composable_stable']}, управляемых {by_family['managed']}. Ещё две регистрации "
                        "не принадлежат этим фабрикам, у них соответственно ноль и один записанный токен. "
                        f"[Номера регистраций, фабрики, токены и резервы](../{sobal_path.as_posix()})."])
        for title, stem in [("Yum Yum Swap","yumyum"),("Bluedex V3","bluedex_v3")]:
            root=Path("tests/data/first_run_dex_2026-11-26")
            native_path=root/(stem+"_base.json")
            module_path=root/(stem+"_base_module.json")
            native, module=read(native_path),read(module_path)
            active=sum(int(m["liquidity"])>0 for m in native["markets"])
            fills=2*len(module["cases"])
            out.extend(["",f"{title} · фабрика подтвердила {len(native['markets'])} V3-пулов, из них с активной ликвидностью {active}. "
                        f"Для всех {active} на блоке 62 262 756 проверены два размера и оба направления. Два официальных роутера "
                        f"выполнили {fills} отдельных обменов, прямой вызов модуля и байткод V5 — ещё {fills}. "
                        "Первый запуск использует `registered_v3`: опрашивает фабрику, immutable fee пула, квотер и TickLens. "
                        "Для `registered_v3_module` владелец развёртывает соответствующий контракт с immutable фабрикой, "
                        "задаёт его адрес в `dex.router` и хеш в `dex.module_runtime_hashes`. "
                        "Модуль сверяет списание собственного входа и реальное получение минимума. Прямое исполнение сравнивается "
                        "с V5 по выплате и полным затратам, включая разрешения. Шаг 17 V5 передаёт точный адрес пула для выбранного fee tier. "
                        "Официальные exactInputSingle возвращают расчётный выход и допускают частичный вход у предела цены. "
                        f"[Фабрика, кошельки и штатные роутеры](../{native_path.as_posix()}) · "
                        f"[Выплаты, минимумы, сроки и остатки разрешений](../{module_path.as_posix()})."])
        out.extend(["", "Memebox · на блоке 62 262 756 фабрика подтвердила 43 CREATE2-пары. В профиле первого запуска "
                    "вид `memebox` читает доступные остатки, физические балансы и тариф с одного блока. У всех 43 пар "
                    "комиссия на вход составила 100 из 10 000, доля протокола — 3000 из 10 000 уже удержанной комиссии. "
                    "Вид `memebox_v5` требует от владельца развернуть MemeboxAdapter с адресом этой фабрики и задать "
                    "`dex.router` и `dex.module_runtime_hashes`. Кошелёк сравнивает прямой вызов и шаг 29 V5 по чистой выплате и полным расходам, "
                    "включая разрешение. Выход ограничен обеспечением пары и фактическим приходом собственного входа. "
                    "На исходном блоке подтверждены 16 исполнений на двух рынках и 8 при внесённом ранее дополнительном остатке. "
                    "Обе оболочки выплатили только сумму за вход плательщика, а повышение минимума на единицу отменило весь обмен. "
                    "[Реестр и границы пары](../tests/data/first_run_dex_2026-11-26/memebox_base.json) · "
                    "[Фактические выплаты, газ и разрешения](../tests/data/first_run_dex_2026-11-26/memebox_base_module.json)."])
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
    if base:
        capture = "tests/data/first_run_dex_2026-11-26/base_v2_factories.json"
        markets = read(Path(capture))
        for id, label, fraction in (("moonbase", "Moonbase", "30/10000"), ("x7-finance-base", "X7 Finance", "200/100000")):
            pools = markets[id]["pools"]
            funded = sum(all(map(int, pool["reserves"])) for pool in pools)
            sample = next(row for row in markets[id]["samples"] if row.get("cases"))
            out.extend(["", f"{label} · {len(pools)} пар подтверждены фабрикой и CREATE2, у {funded} оба резерва положительны. "
                        f"Комиссия обычного swap через V5 — {fraction}. Для пары `{sample['pool']}` "
                        "проверены оба направления на двух объёмах, остатки исполнителя и отказы при превышении выхода на одну единицу. "
                        f"[Архив](../{capture}) · [Исходник пары]({markets[id]['origin']['verified_pair']})."])
        for dex_id, name in [("citadelswap","CitadelSwap"),("kim_v2_base","Kim V2")]:
            file = f"tests/data/first_run_dex_2026-11-26/{dex_id}.json"
            capture = read(Path(file))
            pairs = capture["directory"]["markets"]
            cases = capture["execution"]["cases"]
            out.extend(["", f"{name} · фабрика и CREATE2 подтвердили {len(pairs)} пар, стабильных {sum(p['stable'] for p in pairs)}. "
                        "Профиль camelot_v2 читает две комиссии из getReserves. Для каждого направления архив содержит "
                        f"состояние перед обменом, чистый выход и отказ при превышении минимума на единицу. Полных исполнений V5 {len(cases)*2}. "
                        + ("У пары WETH/0x7233… удержания различаются по входу: 250 и 2000 на 100 000. " if dex_id=="citadelswap" else "В единственной паре на этом блоке менее 0,18 USDC. ")
                        + f"[Фабрика, реестр и измерения](../{file})."])
        out.extend(["", "Sobal · из 78 котировок 8 округлились к нулю. Для всех 70 положительных проверены оба варианта: "
                    "отдельный Vault и V5. Во всех 140 обменах кошелёк получил расчётную сумму, минимальная выплата на единицу выше откатила обмен. "
                    "Для двух продаж DAI начальные средства получены через независимый пул Uniswap. "
                    "[Суммы, газ, начальные разрешения и причины отказов](../tests/data/first_run_dex_2026-11-26/sobal_base_wallets.json.gz).",
                    "", "YumyumAdapter и BluedexAdapter собраны GitHub Actions и проверены на архивном состоянии Base. "
                    "Публичное развёртывание этих сборок владельцем на текущем этапе не зафиксировано. "
                    "Владелец развёртывает модуль с адресом фабрики своей площадки, затем указывает его адрес в профиле DEX. "
                    "Контракты используют разные селекторы обратного вызова. [Проверка обеих сборок](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45908138387)."])
        bunny_path = "tests/data/first_run_dex_2026-11-26/bunnyswap_base.json"
        bunny = read(Path(bunny_path))
        out.extend(["", f"BunnySwap · существует один рынок FRIEND/WETH в RabbitRouter. В {len(bunny['cases'])} размерах "
                    "кошелёк купил FRIEND за ETH и получил ETH при обратной продаже. Комиссия пары 15/1 000; "
                    "дополнительный сбор протокола равен нулю. FRIEND возвращает пустые данные при успешном переводе. "
                    f"[Обе выплаты, границы цены и срока](../{bunny_path})."])
        module_path = "tests/data/first_run_dex_2026-11-26/bunnyswap_base_module.json"
        module = read(Path(module_path))
        out.extend(["", f"При расчёте в WETH для BunnySwap доступны модуль и шаг 29 V5. "
                    f"На том же блоке в {2 * len(module['cases'])} обменах проверены списание и выплата кошелька. "
                    f"Владелец развёртывает BunnySwapAdapter с адресом RabbitRouter, указывает адрес модуля в "
                    f"`dex.router` и выбирает `dex.kind = bunnyswap_module`. "
                    f"Проверенный хеш модуля `{module['module_hash']}`. "
                    f"[Измерения и проверки отказов](../{module_path})."])
        blue_path = "tests/data/first_run_dex_2026-11-26/bluedex_base.json"
        blue = read(Path(blue_path))
        router_samples = sum(2*len(p.get("cases", [])) for p in blue["native_router"]["samples"])
        executor_samples = sum(2*len(p.get("cases", [])) for p in blue["samples"])
        out.extend(["", f"BlueDex V2 · в реестре фабрики {len(blue['pools'])} пары. Пара удерживает 3 из 1 000 единиц входа. "
                    f"Через V5 записано {executor_samples} обменов, через отдельный Router02 — {router_samples}. "
                    f"Роутер проверяет зачисление на кошелёк. [Реестр, комиссии, результаты обеих форм](../{blue_path})."])
        kewl_file = "tests/data/first_run_dex_2026-11-26/kewlswap_base.json"
        kewl = read(Path(kewl_file))
        markets = kewl["execution"]["markets"]
        done = sum(len(m.get("legs", [])) * 2 for m in markets)
        out.extend(["", f"KEWLSwap · все {len(kewl['pools'])} зарегистрированные пары совпали с CREATE-адресами фабрики "
                    f"для nonce от 1 до {kewl['origin']['account_nonce']-1}. Штатный роутер и фабрика находятся на одном прокси "
                    "с разными обработчиками. На трёх парах V5 и роутер дали одинаковую выплату на двух объёмах в обе стороны. "
                    f"Всего {done} измеренных ног, V5 дешевле роутера по газу для этих объёмов. Пара WETH/USDT имеет всего 1 микро-USDT "
                    "в резерве, поэтому купить USDT нельзя. После отдельного получения USDT через Uniswap V2 "
                    "V5 и штатный роутер продали 1 и 2 микро-USDT за 9 098 765 и 12 137 765 wei WETH соответственно. "
                    f"[Создание, роли, проверки кошелька и расход газа](../{kewl_file})."])
        diamond = "tests/data/first_run_dex_2026-11-26/diamondswap_base.json"
        proof = read(Path(diamond))
        liquid = sum(min(map(int, row["balances"])) > 0 for row in proof["markets"])
        out.extend(["", f"DiamondSwap · подтверждены {len(proof['markets'])} CREATE2-пар, у {liquid} ненулевые балансы обоих активов. "
                    "На архивном блоке swapFee() фабрики вернул 20 на 10 000. В паре WETH/USDbC штатный роутер и V5 дали одинаковый чистый выход "
                    "в обе стороны на двух объёмах. При одинаковом разрешении роутер на каждом обмене использовал на 8 317 газа меньше. "
                    "Сделка с другим токеном откатилась. При первом запуске отдельно задаются адрес и код обработчиков фабрики и роутера. "
                    f"Для исполнения через V5 достаточно проверенной роли фабрики и настроенного исполнителя. [Каталог, селекторы, балансы и границы](../{diamond})."])
        candy_file = Path("tests/data/first_run_dex_2026-11-26/candyswap_base.json")
        candy = read(candy_file)
        sides = sum(2*len(row["cases"]) for row in candy["net_payouts"])
        out.extend(["", f"CandySwap · в реестре {len(candy['pools'])} пар. Входная комиссия swapFee() и исходящее удержание "
                    f"селектора 0xc3595a85 читаются на одном блоке. В архиве обе ставки равны 15/10000. "
                    f"Из 837 минимальных единиц USDbC кошелёк получил 836, сбор составил 1. В реестре подтверждены "
                    f"{sides} направленных исполнений V5 на трёх парах, во всех случаях минимум на единицу выше отклонился. "
                    f"При повторном использовании пула расход резерва включает сбор. [Кошелёк и получатель комиссии](../{candy_file.as_posix()})."])
        funded_candy=Path("tests/data/first_run_dex_2026-11-26/candy_module_wallets.json.gz")
        with gzip.open(funded_candy,"rt",encoding="utf8") as handle:
            funded=json.load(handle)
        out.append(f"**CandySwap с учётом собственного взноса.** Владелец развёртывает CandySwapAdapter "
                   f"с фабрикой `{funded['factory']}`, указывает адрес модуля в `dex.router` и выбирает "
                   f"`dex.kind = candyswap_module`. Проверенный `dex.module_runtime_hashes = {funded['module_hash']}`. "
                   f"На трёх рынках зафиксированы {2*len(funded['cases'])} обмена прямым вызовом и через шаг 29 V5. "
                   f"Исходящий сбор входит в выплату и в проверку минимума. Сумма прежних переводов в пул "
                   f"не увеличивает размер заявки. [Архив кошельков](../{funded_candy.as_posix()}).")
        leet = "tests/data/first_run_dex_2026-11-26/leetswap_base_swaps.json"
        out.extend(["", "LeetSwap V2 · фабрика зарегистрировала 4505 волатильных пар и 0 стабильных. У 4196 положительны "
                    "оба сохранённых резерва, у 4115 положительны оба фактических баланса. Профиль leetswap_v2 сверяет баланс "
                    "каждого токена, паузу, общий источник комиссии и текущую ставку. На закреплённом блоке она равна 30/10000. "
                    "Появление внешнего оракула требует сверки его ставки для каждого фактического получателя маршрута. "
                    "Три пары подтвердили 12 направленных исполнений с точной границей выхода. У одной пары два обратных "
                    "обмена отдали меньше формулы из-за перевода токена: допуск использует штатную пробу и налог стороны пула. "
                    "У двух пар с крупными сохранёнными резервами V5 откатился на обоих объёмах. "
                    f"[Каталог и балансы](../tests/data/first_run_dex_2026-11-26/leetswap_base_directory.json.gz) · [Дельты и отказы](../{leet})."])
        out.extend(["", "Классические пары Solidly используют проверенную математику Solidly V1 и шаг 3 исполнителя V5. "
                    "Резервы, ERC-20 балансы пары, источник ставки, код фабрики и для прокси её активная реализация проверяются на одной канонической высоте. "
                    "У пары с расхождением балансов обычная котировка от выделенного ей входа требует отдельного подтверждения. "
                    "Таблица показывает число зарегистрированных пар, совпадение с текущим хешем создания и измеренные исполнения.",
                    "", "| Профиль | Реестр / CREATE2 текущего шаблона / балансы = резервы | Ставка пары | Архив исполнения V5 |",
                    "|---|---:|---|---|"])
        for classic_id in ("hydrex-classic","velocimeter-base","baso-finance","archly-base","archly-v1-base","equalizer-base","infusion"):
            directory_file = Path(f"tests/data/first_run_dex_2026-11-26/base_classic_registry_{classic_id}.json.gz")
            wallets_file = Path(f"tests/data/first_run_dex_2026-11-26/base_classic_wallets_{classic_id}.json.gz")
            with gzip.open(directory_file,"rt",encoding="utf8") as fh:
                directory = json.load(fh)
            with gzip.open(wallets_file,"rt",encoding="utf8") as fh:
                wallets = json.load(fh)
            registered=directory["markets"]
            old=sum(not m["current_init"] for m in registered)
            aligned=sum(m["balances"]==m["reserves"] for m in registered)
            owner="пара" if directory["fee_source"]=="pool" else "фабрика"
            fee_method="getRealFee(address)" if classic_id=="archly-base" else directory["fee_method"]
            curves=", ".join("stable" if m["stable"] else "volatile" for m in wallets["markets"])
            fills=2*sum(len(m["cases"]) for m in wallets["markets"])
            out.append(f"| {classic_id} | {len(registered)} / {len(registered)-old} / {aligned} | {owner} `{fee_method}`, "
                       f"делитель {directory['denominator']} | {curves}, {fills} обменов и столько же отказов при minOut + 1. "
                       f"[Реестр](../{directory_file.as_posix()}) · [Кошельки](../{wallets_file.as_posix()}) |")
        out.extend(["", "На Base Archly v1 и v2 имеют отдельные фабрики. Справочник индексатора выбрал пару из v2, "
                    "обе версии включены через родителя archly-base. Для пула wTV/TV версии v1 второй независимый адрес получил wTV обычным ERC-20 переводом из архивного баланса и выдал своё разрешение. При входах 500000000000000 и 5000000000000000 raw wTV он получил те же 502276010429030 и 5019824065979758 raw TV, а при обратном обмене — 499700055383695 и 4997001430051607 raw wTV. В четырёх случаях порог на единицу выше откатил обмен. Источник Hydrex Classic — getFee() самой пары, "
                    "индивидуальная ненулевая ставка имеет приоритет над общим тарифом фабрики. У Equalizer ставки могут иметь 18 знаков после запятой. "
                    "У Equalizer сверены 32 пары первого поколения, 100 второго и 252 текущего, по два размера и обоим направлениям stable и volatile каждого. Начальные блоки исторических поколений 4334766 и 4938760, при них pairCodeHash() и EIP-1967 реализации связаны с регистрациями CREATE2. Две пары с активным внешним менеджером комиссии требуют отдельной приёмки. [Исторические реализации и кошельки](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45905243473). "
                    "[Размещения Archly](https://docs.archly.fi/security) · [Архивы GitHub](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45903513687)."])
        out.extend(["", "Algebra на Base проверена на четырёх реальных рынках через виртуальный квотер и исполнение V5. В каждом случае измерены два размера покупки и обратная продажа, фактические остатки и отказ при minOut + 1.",
                    "", "| Площадка | Реестр и источник комиссии | Состояние на контрольном блоке | Приёмка |", "|---|---|---|---|"])
        for dex,scope in [("henjin","BasePluginV1, фабрика создаёт плагин каждой пары"),("kim-v4-base","BasePluginV1, фабрика создаёт плагин каждой пары"),("synthswap-v3","Algebra V1.9, одна ставка и несжатый tickTable"),("treble-v4","Algebra Integral с лимитными заявками и условным ребалансированием")]:
            path=Path(f"tests/data/first_run_dex_2026-11-26/base_algebra_{dex}.json.gz")
            with gzip.open(path,"rt",encoding="utf8") as fh: reg=json.load(fh)
            pool=reg["roles"]["pool"]["address"]
            fee=reg["global_state"][2]
            control=reg["reads"].get("getCurrentFee()")
            rate=f"globalState {fee} пипс" + (f", getCurrentFee {int(control,16)}" if control else "")
            if dex in ("henjin","kim-v4-base"):
                verdict="Профиль требует pluginByPool и обратную ссылку, режим 0xc1, нулевой incentive и подтверждённый код обеих фабрик."
            elif dex=="synthswap-v3":
                verdict="Фабрика EternalFarming создала действующий корреспондент фермы при nonce=1. FarmingCenter подтвердил пул и единственную вечную программу. Проверяются код обеих ролей, регистрация пары, CREATE2, семь слов globalState и сырой tickTable. Прогноз использует публичный оракул пула и большую ставку для фактически наблюдаемых переходов времени 0 или 1 секунда."
            else: verdict="В 4 случаях SwapFee сообщил 201 миллионную и нулевую плату плагина при lastFee=500. Подтверждены фабрика плагина, LimitOrderManager и SecurityRegistry, разрешающий обмен. При активной внешней ферме, ребалансировщике или непринятом коде пул закрывается. После пересечения ордера менеджер может сжечь ликвидность того же пула, для повторного использования требуется новое состояние."
            out.append(f"| {dex} | {scope} | {rate}, пул `{pool}` | {verdict} [Вызовы и ответы](../{path.as_posix()}) |")
        out.extend(["", "Virtuals на Base. Канонический блок 62 262 756: Classic зарегистрировал 17 357 пар, Unicorn — 21 877. "
                    "В каждом реестре проверены адреса трёх пар и их остатки. Обе фабрики удерживали 1% на покупке и 1% на продаже. "
                    "Встречный актив VIRTUAL 0x0b3e328455c4059eeb9e3f84b5543f74e24e7e1b; комиссии округляются независимо в его атомах. "
                    "Размер обмена ограничен реальным балансом токена на паре. В Unicorn проверяются launchExecuted и временный стартовый налог. "
                    "После перехода токена на внешний рынок прежняя ссылка Bonding перестаёт действовать. По каждой строке dex.bonding_roles указаны "
                    "прокси, хеш прокси, реализация и хеш реализации. Для Unicorn изучена привязка V4; архивный V2 остаётся без подтверждённого кошелькового рынка. "
                    "Основной вид считывает рынок. Для модульного режима владелец задаёт проверенный адрес dex.router и совместимый runtime. "
                    "В GitHub 45911204066 на двух размерах 0,01/0,1 VIRTUAL фактически получено при обратной продаже 0,009801/0,09801 VIRTUAL до газа. "
                    "Восемь обменов для каждого рынка, включая родные и защищённые, подтвердили дельты обычного кошелька и отказы min+1 и срока. "
                    "На 0,01 VIRTUAL модуль расходовал 342271/316495 gas на Classic и 369928/331428 gas на Unicorn (купить/продать). "
                    "[Выплаты Classic](../tests/data/first_run_dex_2026-11-26/virtuals-base-protected.json.gz) · "
                    "[Выплаты Unicorn](../tests/data/first_run_dex_2026-11-26/virtuals-unicorn-base-protected.json.gz). "
                    "[Реестр Classic](../tests/data/first_run_dex_2026-11-26/virtuals-base-registry.json.gz) · "
                    "[Реестр Unicorn](../tests/data/first_run_dex_2026-11-26/virtuals-unicorn-base-registry.json.gz)."])
        out.extend(["", "HorizonDEX на Base. Kyber Elastic V2, 132 границы в прочитанном списке, порция чтения 32, шаг цены 8, комиссия 40/100000. На блоке 62 262 756 официальный роутер и штатный V5 (шаг 8) дали одинаковые поступления. За 10¹² и 10¹³ wei WETH получены 1688 и 16888 raw USDbC, обратные продажи дали 998650431964 и 9991255908839 wei WETH. Все восемь обменов использовали полный вход. Порог выше на одну единицу и истёкший срок откатили каждый вариант. Штатный путь использует V5 с проверкой выплаты получателю. При наличии отдельного проверенного модуля доступен kyber_elastic_module: dex.elastic_native_router хранит официальный роутер, dex.router — адрес модуля владельца. По 10¹² wei WETH покупка в архиве потребовала 127002 gas напрямую, 140243 через V5 и 189093 через защищённый BasePmmExecutor. Обратная продажа потребовала 112254, 125542 и 185575 gas соответственно. На этой выборке V5 экономит газ среди двух способов с полной проверкой. [Защищённые выплаты](../tests/data/first_run_dex_2026-11-26/horizon-protected.json.gz). [Архив котировок, тиков и выплат](../tests/data/first_run_dex_2026-11-26/horizondex_base.json.gz)"])
        oasis_file = "tests/data/first_run_dex_2026-11-26/oasisswap_base.json"
        oasis_v5_file = "tests/data/first_run_dex_2026-11-26/oasisswap_v5_base.json"
        oasis, oasis_v5 = read(Path(oasis_file)), read(Path(oasis_v5_file))
        cached = sum(any(map(int, row["fee_cache"])) for row in oasis["pools"])
        capped_file = "tests/data/first_run_dex_2026-11-26/oasisswap_v5_capped_base.json"
        rebate_file = "tests/data/first_run_dex_2026-11-26/oasisswap_rebate_base.json"
        capped = read(Path(capped_file))
        out.extend(["", f"OasisSwap · фабрика подтвердила {len(oasis['pools'])} рынков, из них {cached} хранят протокольную комиссию сверх резервов. "
                    "На закреплённом блоке у всех пар ставка 100/10000, доля протокола в комиссии 5000/10000. "
                    "Прямой Router02 проверяет фактическую дельту получателя в варианте с поддержкой налога. Для пары `0x86bcde2d5b06bba5eb9b3c8332f770cdcaf0c95a` "
                    "обе стороны на двух объёмах выполнились также через V5. Для `0xddb5c6730b1720b985dc4547541aedf58d4bdac9` две покупки прошли через V5, "
                    "обратная продажа не прошла на обоих объёмах. Возврат комиссии в нативном пути зависит от получателя, у адаптера — от EOA-отправителя транзакции. "
                    f"Базовый профиль применяет `oasisswap_v2_router`, отдельный модуль V5 использует `oasisswap_v2_v5` и `dex.router` от владельца. "
                    f"GitHub подтвердил скомпилированный runtime `{oasis_v5['module_runtime_hash']}` с адресом этой фабрики. "
                    f"Следующий принятый runtime `{capped['module_runtime_hash']}` обрабатывает ответ estimator выше 10000 по обычной ставке. "
                    "При таком ответе старый адаптер отзывает допуск, а нативный вызов сохраняет useRebate=false. На исходном блоке estimator равен нулевому адресу. "
                    "В изолированной копии проверенных контрактов скидки 0, 1000, 10000 и 10001 дали итоговые ставки 100, 90, 0 и 100 bps. "
                    f"[Новый V5 на архивных рынках](../{capped_file}) · [Границы скидки и откат](../{rebate_file}) · "
                    f"[Рынки и нативные обмены](../{oasis_file}) · [Архив V5](../{oasis_v5_file}) · [GitHub](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45897598960/job/140175260614)."])
        delta_file = "tests/data/first_run_dex_2026-11-26/deltaswap_base.json"
        delta = read(Path(delta_file))
        current, historical = delta["current"], delta["historical"]
        out.extend(["", f"DeltaSwap · в реестре {len(current['pools'])} пар. Ставка обычного Router02 зависит от объёма, "
                    "резервов, двух EMA, суммы обменов внутри блока и порога фабрики. На высоте "
                    f"{current['height']} dsFeeInfo = {current['ds_fee_info']}, на {historical['height']} = {historical['ds_fee_info']}. "
                    "В исторической паре обменов за один блок первая комиссия составила 0, обратная — 0,3% вследствие накопления объёма. Реальные балансы совпали на двух объёмах в обе стороны. "
                    "Роутер проверил минимальную дельту получателя и откатил каждый обмен с минимумом на одну единицу выше. "
                    f"[Архив](../{delta_file}) · [Исходник роутера]({current['proof']['router']['url']})."])
    vista_file = f"tests/data/first_run_dex_2026-11-26/ethervista_{network}.json"
    vista = read(Path(vista_file))
    observation = vista["observations"][-1]
    fees = observation["fees"]
    rows = [p for p in vista["registry"] if vista["weth"] in p["tokens"] and all(map(int,p["reserves"]))]
    scala = vista["fee_wei_per_units"]
    bands = [[int(scala[str(p["fee_units"][side])]) for p in rows] for side in (0,1)]
    same = sum(p["balances"] == p["reserves"] for p in vista["registry"])
    backing = (f"Оба фактических баланса совпали с резервами у {same}/{len(vista['registry'])} пар. "
               + ("" if base else "У пары UNI `0xf6f275d6fd242dbd10d6c026808e2918009be90f` избыток 100000000000000 raw UNI, "
                  "у пары `0x3b0da15b994745a164905c06e6df7dca55b928d8` сохранён резерв токена 10²⁷ при балансе 10⁶ raw. "
                  "У `0xf6fc63c8482416d3f1a5e89281a713e21724ddfa` не получено значение баланса первого токена. "))
    owner_note = ("На Base модуль обращается к фабрике Base и местному WETH. "
                  if base else "На Ethereum этот же интерфейс использует другую фабрику и WETH основной сети. ")

    out.extend(["", f"EtherVista · фабрика подтвердила {len(vista['registry'])} пар, из них у {vista['funded_native_pairs']} "
                "есть обёрнутый ETH и два положительных резерва. Для каждой пары сохранены buyTotalFee и sellTotalFee, "
                "а также перевод каждого уникального значения через usdcToEth активного роутера. "
                f"На проверенном рынке `{observation['pool']}` покупка стоила {fees['buy']['native_wei']} wei протокольной комиссии, "
                f"продажа — {fees['sell']['native_wei']} wei. Газ учтён отдельно. "
                f"Среди пар с WETH и положительной ликвидностью диапазон платы за покупку {min(bands[0])}–{max(bands[0])} wei, "
                f"за продажу {min(bands[1])}–{max(bands[1])} wei. Нулевой платёж был у {bands[0].count(0)} рынков на покупке "
                f"и {bands[1].count(0)} на продаже. {backing}{owner_note}"
                "Значение owner() — владельцем указанная при развёртывании роль пополнения и вывода ETH-резерва. "
                "Адрес торгового EOA и владелец резерва могут различаться. В V5 покупка использует входной WETH, "
                "продажа платит fee из возобновляемого feeReserve, а затем восполняет резерв из выручки. "
                "При нулевом резерве покупка сохраняется, продажа через swapFrom требует покрытия sell fee. "
                "У отдельного native-вызова кошелёк дополнительно передаёт плату ETH в msg.value. "
                f"Фабрика разрешает `{vista['router']}`. Прежний `{vista['previous_router']}` сохранил ссылку на ту же фабрику "
                "и отклоняется при проверке обратной связи. Профиль первого запуска использует официальный роутер. "
                "Адрес адаптера и хеш его развёрнутого кода с прошитыми владельцем и размещением задаются владельцем для режима V5. "
                f"[Реестр и реальные дельты кошелька](../{vista_file}) · [GitHub](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45898296581)."])
    routes_file = f"tests/data/first_run_dex_2026-11-26/ethervista_v5_{network}.json.gz"
    with gzip.open(routes_file, "rt", encoding="utf8") as stream:
        routes = json.load(stream)
    native_row = next(r for r in routes if r["native"])
    if native_row["selector"] != vista["selector"]:
        raise ValueError("Архив выплаты EtherVista относится к другому блоку")
    small, large = native_row["native"]
    net = large["expected_sell_net"]
    router_gas = int(large["sell_router_self"]["gas"],16)
    module_gas = int(large["sell_module_self"]["gas"],16)
    out.extend(["", f"Для пары `{large['pool']}` при продаже стейблкоина за ETH меньший архивный размер дал "
                f"{small['expected_sell_net']} wei после протокольной платы. При большем размере тому же кошельку "
                f"поступило {net} wei после аванса и до газа. Официальный роутер израсходовал {router_gas} ед. газа, "
                f"модуль с восстановлением остатка — {module_gas}. В отправляемом официальном вызове валовой минимум "
                "равен чистому минимуму плюс аванс. Положительный исходный остаток WETH у роутера исключает этот вариант. "
                "Если расчётной валютой выбран ETH, проверяются совпадение тикера и адреса обёртки и реальная выплата ETH "
                "на кошелёк. Для зачисления USDT требуется фактически исполненная конверсия. "
                f"[Архив адресов, вызовов и событий](../{routes_file}) · "
                "[GitHub Actions](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45901130658)."])
    vista_volume_file = f"tests/data/first_run_dex_2026-11-26/ethervista_volume_{network}.json"
    turnover = read(Path(vista_volume_file))
    if turnover["last_hash"] != vista["selector"]["blockHash"] or turnover["registered_pairs"] != len(vista["registry"]):
        raise ValueError(f"Числа EtherVista относятся к иному блоку или реестру: {network}")
    whole, frac = divmod(int(turnover["weth_turnover_wei"]), 10**18)
    weth_total = f"{whole}.{frac:018}".rstrip("0").rstrip(".")
    out.extend(["", f"За 24 часа после {turnover['after_utc']} по {turnover['through_utc']} включительно "
                f"на блоках {turnover['first_block']}–{turnover['last_block']} реальные Swap-события EtherVista дали {weth_total} WETH оборота. "
                f"Вход составил {turnover['weth_in_wei']} wei WETH, выход {turnover['weth_out_wei']} wei WETH. "
                f"Событий {turnover['swaps']}, транзакций {turnover['transactions']}, задействованных пар {turnover['active_pairs']} из {turnover['registered_pairs']}. "
                "Для оборота суммируется одна сторона WETH каждого обмена. "
                f"[События и канонические границы интервала](../{vista_volume_file}) · [GitHub](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45900157409)."])

    nami = read(Path(f"tests/data/first_run_dex_2026-11-26/nami_{network}.json"))
    samples = ", ".join(f"`{m['pool']}` — {m['tick_window']['measured_tick_steps']}" for m in nami["markets"])
    out.extend(["", f"Nami · фактический executor V5 выплатил полный результат {sum(len(m['cases']) for m in nami['markets'])} архивных обменов. "
                f"На этих пулах окно тиков потребовало следующее число шагов в каждую сторону: {samples}. Лимит dex.integral_tick_steps задаёт владелец. "
                "Наблюдаемый pluginConfig равен 195, оба alpha равны 0, сегментный источник выключен и cap равен 0. "
                "Проверенные исходники в этом режиме дают обычному отправителю baseFee; площадка сверяет обе стороны swapFee, обе привязки фабрики, "
                "слот beacon, runtime реализации и расширения. Отдельный обмен Nami сравнивается по сумме и газу с обычными вариантами маршрута. "
                f"[Архив пула, двух направлений и полных дельт](../tests/data/first_run_dex_2026-11-26/nami_{network}.json)"])
    v2_probes = Path("tests/data/base_ethereum_v2_probes.json")
    fixed = [row["dex"] for row in read(v2_probes) if row["network"] == network
             and row.get("verify_pair_boundary") and len(row.get("cases", [])) == 2]
    if fixed:
        out.extend(["", "Для " + ", ".join(sorted(fixed)) +
                    " источник комиссии подтверждён кодом пары, а происхождение пула проверено по CREATE2. "
                    "На двух размерах в обоих направлениях пул отдал расчётный выход; запрос ещё одной минимальной "
                    "единицы отвергнут инвариантом самого пула. Исполнитель отдельно подтвердил движения кошелька. "
                    f"[Блоки, пулы, объёмы и причины отката](../{v2_probes.as_posix()})."])
    if not base:
        registry_path = Path("tests/data/first_run_dex_2026-11-26/ethereum_v2_factories.json")
        registered = read(registry_path)
        counts = ", ".join(f"{dex} {len(row['pools'])}" for dex, row in registered.items())
        out.extend(["", f"Пять фабрик V2 Ethereum. Все адреса проверены по обоим getPair и CREATE2: {counts}. "
                    "На одном пуле каждой фабрики подтверждены списание, выплата, возврат и цена ещё одной единицы на двух размерах. "
                    "Первый испробованный пул JustMoney отклонил обмен. Проверка сделки через такую пару по-прежнему требуется. "
                    f"[Реестры, расчёты и исходные отказы](../{registry_path.as_posix()})."])
    variable_path = Path("tests/data/base_ethereum_v2_variable_fees.json")
    variable = [r for r in read(variable_path) if r["network"] == network]
    if variable:
        out.extend(["", "Изменяемая комиссия V2 читается на блоке резервов, в том числе перед обменом. "
                    "На архивном блоке контракты вернули " +
                    ", ".join(f"{r['dex']} {r['fee_fraction'][1]}/{r['fee_fraction'][0]} "
                              f"({r['fee_getter']})" for r in variable) +
                    ". Для каждой фабрики проверены два размера в обоих направлениях, "
                    "включая прямой отказ пула при увеличении максимального выхода на единицу. "
                    f"[Вызовы и движения средств](../{variable_path.as_posix()})."])
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
            router_runtime = next(x["runtime_keccak"] for x in data_fermi["historical"] if x["router"] == data_fermi["router"])
            out.extend(["", "| Настройка eth_dex для Fermi | Проверенное значение |", "|---|---|",
                        *[f"| `fermi.{key}` | `{value}` |" for key,value in
                          [("router",data_fermi["router"]),("engine",data_fermi["engine"]),
                           ("router_runtime_hash",router_runtime),("executor_runtime_hash",wrapped["runtime_keccak"])]]])
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
    kipseli = Path("tests/data/first_run_dex_2026-11-26/kipseli_base.json")
    elfomo = Path("tests/data/first_run_dex_2026-11-26/elfomofi_base.json")
    if base and elfomo.exists():
        el = read(elfomo)
        ok = sum(c["status"] == "filled" for c in el["cases"])
        zero = sum(c["status"] == "zero_quote" for c in el["cases"])
        out.extend(["", f"ElfomoFi · router `{el['router']}`, vault `{el['vault']}`. У pricing "
                    f"`{el['pricing']}` на этом блоке реализация `{el['pricing_implementation']}`. "
                    f"getSupportedPairs вернул {len(el['pairs'])} пар. У WETH/USDC и cbBTC/USDC {ok} исполнений "
                    "с partnerId=0 дали полные встречные дельты на плательщике и vault. Контракт вернул пустые байты. "
                    f"У USDT/USDC {zero} нулевых котировок. Для остальных пар двусторонняя проверка отсутствует. "
                    "Профиль elfomofi строит сигнальную лестницу только WETH/USDC и cbBTC/USDC. Каждый запрос цены "
                    "передаёт фактический адрес кошелька с partnerId=0. Покупка и продажа имеют отдельные ступени. Для продажи getAmountIn предлагает размер "
                    "входа в монете, getAmountOut фактического плательщика подтверждает выплату. В [архивной проверке шести состояний](../tests/data/first_run_dex_2026-11-26/elfomo_ladders_base.json) "
                    "при исчерпании одной стороны сохранились обе ступени встречного направления. Платёжная ликвидность ограничена остатком vault "
                    "и его allowance. Выход, достигший всего доступного остатка, отсекается. Размеры dex.pmm_ladder_usd "
                    "задаёт владелец в USDT, dex.pmm_ladder_counter переводит эти объёмы в USDC по курсу реестра. "
                    "Нативный swap сравнивает limitAmount с котировкой до transferFrom. Архивная проверка ElfomoExecutor подтвердила срок, полный вход, зачисление и возврат разрешения к нулю в 8 случаях. В режиме elfomofi_module котировка кошелька проходит через адрес адаптера. Для единственного нового размещения по решению 408 у ElfomoFi и LunarBase общий BasePmmExecutor, runtime 0x6587e95f1ad53b3a542bd2fd8f9b81d6e8f2e0fdc0e0d73440e77d0e31a56f24. Предыдущий двухрыночный runtime 0xce587f0be477a2a6a9bd8b72a5123b9474709cf99c4c958cf777e698d7284403 совместим с ElfomoFi и LunarBase. Старый ElfomoExecutor 0xd3abddebdd155ff956b970579304da391511f945803c0ae5f1ec8f28c10a64bb допускается только для ElfomoFi. За отдельные 24 часа эти две пары дали 2 094 922,843102 из 2 132 483,689949 USDC оборота роутера. [Расчёт стоимости и границы применения](DECISIONS.md#дополнение-к-t488-01-elfomofi-на-base-dex-23-r-22). "
                    f"[Сырые результаты](../{elfomo.as_posix()}) · [Спецификация]({el['source']})."])
    baibai = Path("tests/data/first_run_dex_2026-11-26/baibai_base.json")
    if base and baibai.exists():
        bai = read(baibai)
        out.extend(["", "Для ElfomoFi, LunarBase и BaiBai адреса рабочих контрактов и токенов, реализации прокси и хеши runtime "
                    "берутся из настроек соответствующего DEX. Таблица первого запуска предлагает значения архивного снимка. "
                    "dex.pmm_counter — встречная монета; dex.pmm_base_tokens — базовые монеты с полными обменами в архиве. "
                    "В dex.pmm_runtime_hashes у ElfomoFi идут router, pricing proxy, implementation; у LunarBase — pool proxy, implementation; "
                    "у BaiBai — общий runtime прокси, затем реализации Entry, CurveBook, Custodian. В dex.pmm_aux_implementations у BaiBai "
                    "сначала адрес реализации CurveBook, затем Custodian. dex.pmm_pricing_offsets и dex.pmm_vault_offsets у ElfomoFi привязывают эти роли к словам PUSH32 в runtime. "
                    "Другое размещение проверяется по его фактическому коду, слотам и ABI на одном блоке. Для *_module dex.pmm_module_bindings сопоставляет фактические байты PUSH рынка и vault в runtime исполнителя с рабочими ролями. Старая ссылка BaiBai на один контракт соответствует рынку dex.native_wrapped."])
        out.extend(["", "BaiBai · профиль WETH/USDC и NVDAc/USDC сверяет три proxy, их реализации и взаимные ссылки. "
                    "quoteFor читает чистую сумму для фактического плательщика. На контрольном блоке TTL книги составил 8 секунд. "
                    "В четырёх архивных обменах точный минимум совпал с зачислением на кошелёк, а минимум на единицу выше привёл к отказу. "
                    "После истечения срока при прежнем содержимом pair() все четыре котировки обнулились и обмены отклонились. "
                    "Расход составил 149 386–151 249 gas. Размеры ступеней задаёт владелец в USDT. Профиль работает в режиме наблюдения. "
                    "За 24 часа учтены 2 471 обмен WETH/USDC на 721 513,217973 USDC из 751 652,107616 USDC всего Entry. "
                    "Ещё 203 обмена дали 30 138,889643 USDC по NVDAc/USDC. NVDAc (8 знаков), 0xb20000000000000000000078ee7ce2fe4908108c, — сертификат NVIDIA CB от COINBASE ONCHAIN SPV LTD. У нативного B20 на этом блоке исполнителя, отправителя и получателя проверяла политика 5. Обратная продажа всей полученной суммы проверена одним публичным кошельком. Для кошелька владельца и биржевого ввода/вывода подтверждения отсутствуют. baibai_module требует отдельного допуска и общего адреса dex.router. [Эмитент, биржи и границы допуска](DECISIONS.md#дополнение-к-t488-01-рынки-baibai-wethusdc-и-nvdacusdc-на-base-dex-25-r-24). "
                    f"[Адреса, котировки и исполнение](../{baibai.as_posix()}) · "
                    "[Оборот по событиям](../tests/data/first_run_dex_2026-11-26/baibai_volume_base.json)."])
    lunar = Path("tests/data/first_run_dex_2026-11-26/lunarbase.json")
    if base and lunar.exists():
        lb = read(lunar)
        market = lb["markets"][2]
        out.extend(["", f"LunarBase · у proxy `{market['address']}` в EIP-1967 получена реализация `{market['implementation']}`. "
                    f"В опубликованном mainnet/addresses.json 0.4.1 указан `{lb['official']['implementation']}`. "
                    "Из четырёх размещений каталога три были на паузе с устаревшей ценой. В действующем пуле X — native ETH, Y — USDC. "
                    "Для четырёх обменов публичного кошелька фактические балансы совпали с quoteExactIn; minOut + 1 и просроченный deadline отклонились. "
                    "Цена зависит от вызывающего адреса. Профиль lunarbase показывает котировку прямого кошелька; в режиме lunarbase_module расчёт идёт через проверенный модуль, "
                    "который принимает WETH и сам оборачивает либо разворачивает ETH. Один общий BasePmmExecutor прошёл 4 обмена LunarBase и 8 ElfomoFi. В сборке 2f2115ff те же четыре обмена LunarBase прошли на общем runtime. Размещение обновлённого модуля оценено в 1 607 490 gas. За 24 часа оборот единственного живого пула составил 1 458 887,970774 USDC. Для рабочего режима нужен допуск владельца. [Архив, стоимость и условия](DECISIONS.md#дополнение-к-t488-01-общий-модуль-basepmmexecutor-для-elfomofi-и-lunarbase-dex-24-r-23). "
                    f"[Сырые вызовы и ответы](../{lunar.as_posix()})."])
    aero_basic = Path("tests/data/first_run_dex_2026-11-26/aero_basic_base.json")
    if base and aero_basic.exists():
        ab = read(aero_basic)
        for f in ab["factories"]:
            markets = f["markets"]
            trials = sum(len(m["cases"]) for m in markets)
            out.extend(["", f"Aero Basic {f['curve']} · {f['count']} пулов в allPools() фабрики. "
                        f"У {len(markets)} проверенных пулов getPool вернул адрес клона реализации `{f['implementation']}`. "
                        f"Комиссии в единицах 1/10000 по порядку снимка: {', '.join(str(m['base_fee']) for m in markets)}. "
                        f"У получателя сошлись списания и выплаты на {trials * 2} обменах типа AERO_BASIC (26), "
                        "включая обратные обмены в том же блоке. minOut на единицу выше отвергнут в обоих направлениях. "
                        f"[Код, реестры, комиссии и вызовы V5](../{aero_basic.as_posix()}) · [Адреса фабрик]({ab['source']})."])
    if base and kipseli.exists():
        k = read(kipseli)
        calls = {r["signature"]: r for r in k["calls"]}
        target = lambda sig: calls[sig]["params"][0]["to"]
        address = lambda sig: "0x" + calls[sig]["response"]["result"][-40:]
        out.extend(["", f"Kipseli PropAMM · Base {k['blockNumber']}. Router `{target('swapImpl()')}`, "
                    f"реализация на блоке `{address('swapImpl()')}`, QuoteLens `{target('getQuoteToken()')}`, "
                    f"валюта котировки USDC `{address('getQuoteToken()')}`. QuoteLens общедоступен. "
                    f"Рыночные данные GET /v2/price без X-API-KEY вернули HTTP {k['public_get']['status']}. "
                    "Для on-chain quote нужны согласованный с командой адрес подписанта EIP-712 и метка времени "
                    "в пределах 10 секунд. Для исполнения требуется доступ к POST /v2/swap/sign и полученные "
                    "verificationData и timestamp. Владелец запрашивает допуск через Contact Us на сайте проекта. "
                    "Проверка реального выхода для кошелька ожидает предоставления доступа. "
                    f"[Сырые ответы](../{kipseli.as_posix()}) · [Документация и выдача доступа]({k['docs']}) · "
                    f"[Контакт команды]({k['contact']})."])
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
        wrapper = solidly.with_name(f"solidly_v3_executor_{network}.json")
        if wrapper.exists():
            accepted = read(wrapper)
            full_count = sum(c["full"] for c in accepted["cases"])
            out.extend(["", f"SolidlyV3Executor · runtime Keccak `{accepted['runtime_keccak']}`, "
                        f"канонический блок `{accepted['selector']['blockHash']}`. Проверено полных обменов "
                        f"{full_count}, атомарных отказов при неполном входе {len(accepted['cases']) - full_count}. "
                        "По завершении каждого вызова вход и выход на модуле, а также разрешение модуль → пул равны нулю. "
                        "При превышении фактической выплаты на одну минимальную единицу каждый вызов отклонился. "
                        f"[Вызовы и балансы получателя](../{wrapper.as_posix()}) · "
                        "[Артефакт компиляции](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45878675517)."])
        if not base:
            out.extend(["", "Ethereum · у USDC/WETH из бюджета 1 WETH пул списал 0,219192077251805298 WETH "
                        "и выплатил 204,192073 USDC. У USDC/USDT без ограничения газа quoteSwap на 10 USDT "
                        "показал расход 8,137669 USDT и выход 8,113045 USDC. При пределе 16 777 216 gas вызов "
                        "превысил лимит. Для DEFROGS/WETH проверены обе стороны обмена на двух размерах."])
    lista = "tests/data/first_run_dex_2026-11-26/lista_ethereum.json"
    lista_stable = "tests/data/first_run_dex_2026-11-26/lista_stable_ethereum.json"
    if base:
        out.extend(["", "9mm V3, Throne V3 и Omni V3 на Base. Для собственного кошелька подтверждён общий модуль 0x6587e95f1ad53b3a542bd2fd8f9b81d6e8f2e0fdc0e0d73440e77d0e31a56f24. "
                    "При настройке `dex.kind=base_native_v3_module` указывается его адрес в `dex.router`. Предустановленный режим `base_native_v3` наблюдает рынок. "
                    "Роутер каждой фабрики оплачивает фактически потреблённый вход. Модуль сверяет полный расход и чистую выплату кошельку, включая перевод токена. "
                    "На блоке 62 262 756 в семи из девяти сценариев прошли обе стороны. При лимите 16 777 216 газа через Omni прошли дополнительные круги с входом 0,05 и 0,10 USDC, а также 0,05, 0,10 и 1 frxUSD. Исходные frxUSD получены в другом пуле за 12 USDC. Каждый из десяти обменов проверен по поступлению в кошелёк через роутер и общий модуль, с откатом при minOut+1. Из 1 frxUSD поступило 0,999945 USDC, после обратного обмена — 0,999799709272636829 frxUSD. Для входа 1 и 10 USDC и 10 frxUSD квотер вернул Unexpected error. Причина внутри пула не установлена. "
                    "Проверены события создания и getPool для 896 пулов 9mm, 24 Throne и 50 Omni. Настройки охватывают все использованные ставки на этом блоке, включая 20 000 у 83 пулов 9mm и 50 у 10 пулов Omni. "
                    "[Платежи, комиссии пулов и адреса](../tests/data/first_run_dex_2026-11-26/base_native_v3.json) · "
                    "[Фабрики и TickLens](../tests/data/first_run_dex_2026-11-26/base_native_v3_profiles.json) · [970 пулов из событий фабрик](../tests/data/first_run_dex_2026-11-26/base_native_v3_directory.json)."])
    derp_path = Path("tests/data/first_run_dex_2026-11-26/derpdex_markets.json")
    fee_file = Path("tests/data/first_run_dex_2026-11-26") / ("v3_fee_state_base.json.gz" if base else "v3_fee_events_ethereum.json.gz")
    if fee_file.exists():
        fees = json.loads(gzip.decompress(fee_file.read_bytes()))
        out.append(f"**Комиссии фабрик V3.** Для {len(fees['factories'])} фабрик прочитаны исходные квитанции создания "
                   f"и проверены {len(fees['probed_fees'])} значений через `feeAmountTickSpacing` на блоке {fees['block']}. "
                   "Все найденные положительные ставки включены в профили первого запуска. "
                   "На Base дополнительные регистрации вне проверенного набора возможны. "
                   "Список применяется к поиску пулов по фабрике при включённом `index.factory_scan`. "
                   f"[Исходные события, шаг тика и проверенные ставки](../{fee_file.as_posix()}).")
    fork_file = fee_file.with_name(f"fork_fee_registry_{network}.json.gz")
    if fork_file.exists():
        archive = json.loads(gzip.decompress(fork_file.read_bytes()))
        summaries = []
        for factory in archive["factories"]:
            tiers = {int(f) for f in factory["tiers"]}
            used = {m["fee"] for m in factory["markets"]}
            unused = ", ".join(str(f) for f in sorted(tiers - used))
            summaries.append(f"`{factory['id']}` — {len(factory['markets'])} пулов, {len(tiers)} ставок"
                             + (f", без пулов на ставках {unused}" if unused else ""))
        out.append(f"**Регистрация ставок iZiSwap и Kyber Elastic.** На блоке {archive['block']} полный корень хранилища "
                   "каждой фабрики подтверждает зарегистрированные пулы и ставки, включая ещё ни разу не использованные. "
                   + ". ".join(summaries) + ". Для каждой ставки сохранены ответы до и после её активации. "
                   "У Kyber исходная квитанция содержит SwapFeeEnabled. У iZiSwap и BitSwap регистрация проходит без события. "
                   f"[Доказательства обеих фабрик](../{fork_file.as_posix()}).")
    if not base and "bancor-v21-ethereum" in configured:
        file = Path("tests/data/first_run_dex_2026-11-26/bancor_v21_wallet.json.gz")
        wallet = json.loads(gzip.decompress(file.read_bytes()))
        cases = wallet["controls"]
        two = {(tuple(r["path"]), r["spent"], r["mode"]): r for r in cases if len(r["path"]) == 5}
        saved = []
        for (path, spent, mode), row in two.items():
            if mode == "wallet":
                v5 = two[path, spent, "v5"]
                assert v5["source"] == row["source"] and v5["target"] == row["target"] and v5["received"] == row["received"]
                saved.append(v5["gas"] - row["gas"])
        out.append(f"**Bancor V2.1.** Прямой вызов может провести обмен через один конвертер или через два по общему промежуточному активу. "
                   "Ставку рассчитывает оригинальный BancorNetwork по явному пути. "
                   f"На архивном блоке прошли {sum(r['mode']=='wallet' for r in cases)} выплат по ERC20/WETH, "
                   f"{sum(r['mode']=='native_wallet' for r in cases)} по нативной валюте и "
                   f"{sum(r['mode']=='v5' for r in cases)} через V5. В проверку вошли пути через один и два пула. "
                   f"В {len(saved)} сравнениях при одинаковых входе и выплате один вызов Network через модуль "
                   f"потребовал на {min(saved)}–{max(saved)} единиц газа меньше, чем два шага V5. "
                   "Величина в USDT зависит от стоимости газа и курса ETH при отправке. "
                   "После размещения модуля из карточки DEX сохраните его адрес в `dex.router`. "
                   f"[Исходные суммы, квитанции, газ и отклонённые поручения](../{file.as_posix()}).")
    if base and "alien-carbon-base" in configured:
        file = Path("tests/data/first_run_dex_2026-11-26/alien_carbon_base.json.gz")
        archived = json.loads(gzip.decompress(file.read_bytes()))
        carbon = archived["original"]
        pairs = carbon["directory"]
        book = sum(p["count"] for p in pairs)
        out.append(f"**Alien Base Carbon.** На блоке {carbon['height']} ваучер указал контроллер "
                   f"`{carbon['roles']['proxy']['address']}`. В нём {len(pairs)} пар, {book} стратегий, "
                   f"в {sum(p['count']>0 for p in pairs)} парах есть стратегии. Прямой вызов реализации вернул "
                   f"{carbon['implementation_pair_count']} пар. Комиссия по умолчанию {carbon['default_fee']} ppm, "
                   "действующая комиссия берётся для каждой пары. Обычная заявка и разделение по двум стратегиям "
                   f"проверены прямой оплатой и через CarbonWallet. Для защищённого пути оператор размещает CarbonWallet "
                   "из карточки DEX в сети Base и сохраняет подтверждённый адрес в `dex.router`. "
                   f"[Кошелёк, суммы, комиссии и реестр](../{file.as_posix()}).")
    if base and derp_path.exists():
        directory_path = derp_path.with_name("derpdex_directory.json")
        directory = read(directory_path)
        capture = read(derp_path)
        markets = capture["markets"]
        quotes = [q for m in markets for q in m["quotes"]]
        full = sum(q.get("full", False) for q in quotes)
        zero = sum(q.get("out") == "0" for q in quotes)
        charged_zero = sum(q.get("out") == "0" and q.get("used") == q["requested"] for q in quotes)
        partial = sum(0 < int(q.get("out", 0)) and not q["full"] for q in quotes)
        failed = sum("reason" in q for q in quotes)
        widened = sum(q.get("larger_gas", {}).get("full", False) for q in quotes)
        rounds = sum("sold" in w for m in markets for w in m["wallet"])
        more = sum("sold" in w for m in markets for w in m.get("larger_gas_wallet", []))
        covered = sum(any("sold" in w for w in m["wallet"]) for m in markets)
        existing = sum("sold" in w for m in markets for w in m.get("existing_holder", {}).get("wallet", []))
        height = f"{directory['to']:,}".replace(",", " ")
        out.extend(["", f"**DerpDEX, Base.** На блоке {height} фабрика зарегистрировала {len(markets)} пулов "
                    f"при {len(directory['tiers'])} ставках комиссии. Корень хранилища фабрики воспроизведён по всем "
                    f"ненулевым значениям ({directory['storage_slots']}). В открытом индексе GeckoTerminal были "
                    f"{len(directory['indexed'])} из этих пулов. "
                    f"[Реестр, тарифы и доказательство состояния](../{directory_path.as_posix()}).",
                    "", f"Для каждого направления проверены четыре размера. При пределе 1 200 000 gas получены "
                    f"{full} полных котировок с выплатой, {partial} положительных частичных, {zero} с нулевым выходом и {failed} откатов. "
                    f"В {charged_zero} котировках с нулевым выходом пул принял весь вход. "
                    f"Из откатов ещё {widened} вернули полный объём при 16 777 216 gas. Изолированный кошелёк, "
                    f"получивший вход из ETH, выполнил {rounds} кругов покупки и обратной продажи на {covered} рынках "
                    f"и ещё {more} круга с повышенным лимитом. В каждом проверены остатки и откат каждого плеча "
                    f"при повышении minOut на единицу. "
                    + (f"В паре GM/ETHEREUM отдельно прошли {existing} круга с существующим ERC-20 балансом "
                       "исторического отправителя. Пополнение этих токенов из WETH в данном опыте не подтверждено. "
                       if existing else "")
                    + f"[Исходные запросы по размерам, котировки и балансы](../{derp_path.as_posix()})."])
    if base:
        aquila_path=Path("tests/data/first_run_dex_2026-11-26/rubicon_aquila_base.json.gz")
        with gzip.open(aquila_path,"rt",encoding="utf8") as stream:
            aquila=json.load(stream)
        out.extend(["",f"Rubicon Aquila V2 · в пуле {aquila['pool']} сверены {len(aquila['cases'])} выплат "
            "штатного Router и те же две покупки и продажи через V5. Для каждого вызова измерены изменения "
            "балансов владельца. Завышенный минимум и истёкший срок вызвали откат. "
            f"[Котировки и расчёты на одном блоке](../{aquila_path.as_posix()})."])
        cycle=Path("tests/data/first_run_dex_2026-11-26/base_usdt_cycle.json.gz")
        with gzip.open(cycle,"rt",encoding="utf8") as stream:
            trials=json.load(stream)
        out.extend(["",f"Циклы с расчётом в USDT · {len(trials['cases'])} архивных исполнений по четырём шагам. "
            "Входы 1 и 100 USDT. Во всех случаях выплата ниже вложенного ещё до газа. "
            "`dex_dex.cash_probe_native = 0.001` ETH служит для оценки газа при поиске. Перед отправкой "
            "проверяется полная стоимость измеренного газа в USDT. "
            f"[Состояние пула, кошелька, подписываемый минимум и выплаты](../{cycle.as_posix()})."])
    if not base:
        archive = Path("tests/data/first_run_dex_2026-11-26/uniswap_v1_ethereum.json.gz")
        with gzip.open(archive, "rt", encoding="utf8") as handle:
            v1 = json.load(handle)
        paid = sum("spent" in case for case in v1["cases"])
        out.extend(["", f"Uniswap V1 · Ethereum, блок {v1['block']}. Фабрика зарегистрировала {v1['count']} токенов. "
                    f"В архиве проверены {len(v1['markets'])} биржи, {paid} обменов с выплатой. "
                    "В исходном пуле USDT на этом блоке нулевой резерв ETH. "
                    "Без модуля кошелёк использует физический ETH или два проверенных ERC20. "
                    "Для защищённого расчёта в ETH, WETH и шага V5 укажите в dex.router адрес UniswapV1Adapter, "
                    "развёрнутого с dex.factory и dex.native_wrapped. "
                    f"[Баланс до и после каждой операции](../{archive.as_posix()})."])
    if not base:
        fixture=Path("tests/data/first_run_dex_2026-11-26/yield_basis_wallet.json.gz")
        with gzip.open(fixture,"rt") as stream:
            book=json.load(stream)
        cases=book["cases"]
        success=[case for case in cases if case["stage"]=="paid"]
        out.extend(["",f"YieldBasis · Ethereum, блок {book['block']}. Профиль yield-basis-ethereum связывает каждую версию LEVAMM с её правилом проверки долга. "
            f"Оригинальный минимум подтверждён кошельком в {sum(c['mode']=='wallet' for c in success)} обменах ERC20, "
            f"в {sum(c['mode']=='native' for c in success)} обменах с ETH и в {sum(c['mode']=='v5' for c in success)} обменах через V5. "
            f"В {sum(c['stage']=='pool rejected' for c in cases)} проверках исходный пул отклонил обмен и баланс владельца сохранился. "
            "Для размещения у семейства выберите YieldBasisAdapter в Ethereum. Сохраните подтверждённый адрес в dex.router. "
            f"[Кошельковые суммы и расходы газа](../{fixture.as_posix()})."])
        routes=fixture.with_name("yield_basis_usdt_routes.json.gz")
        with gzip.open(routes,"rt") as stream:
            paid=json.load(stream)["rows"]
        gas=[row["gas"] for row in paid]
        approval=sorted({row["approval_gas"] for row in paid})
        out.extend(["",f"Curve USDT/crvUSD и YieldBasis WETH/crvUSD провели {len(paid)} обменов через V5 одним вызовом. "
            "При покупке вход составлял 2 или 10 USDT, при продаже — 0,001 или 0,005 WETH. "
            f"Газ двух шагов составил {min(gas)}–{max(gas)} единиц, первоначального разрешения — {approval[0]} или {approval[-1]}. "
            "При окончательном минимуме на один атом выше выплаты обе ноги откатывались. "
            f"[Движения USDT, промежуточного crvUSD и WETH](../{routes.as_posix()})."])
    if not base:
        original_path=Path("tests/data/first_run_dex_2026-11-26/balancer_v1_ethereum.json.gz")
        paid_path=original_path.with_name("balancer_v1_wallet.json.gz")
        if paid_path.exists():
            with gzip.open(original_path,"rt") as stream:
                pools=json.load(stream)
            with gzip.open(paid_path,"rt") as stream:
                cash=json.load(stream)
            rows=pools["payments"]
            out.extend(["", f"Balancer V1 · Ethereum, блок {pools['block']}. Фабрика подтвердила {len(pools['registered'])} оригинальных BPool. "
                f"На интервале блоков {pools['interval'][0]}–{pools['interval'][1]} у {len(pools['markets'])} пулов было {pools['original_events']} событий обмена. "
                f"Из {len(rows)} проверенных сумм на исходных контрактах получено {sum('spent' in r for r in rows)} выплат. "
                f"В {sum(r.get('pool_revert')=='ERR_MATH_APPROX' for r in rows)} случаях формула вернула положительное число, а сам пул отклонил обмен по ERR_MATH_APPROX. "
                f"Через модуль и V5 воспроизведено {len(cash['cases'])} выплат. Адрес модуля, проверенного по dex.module_runtime_hashes, сохраняется в dex.router. "
                "Конструктор BalancerV1Adapter принимает адреса BFactory и WETH. "
                f"[Реестр и исходные выплаты](../{original_path.as_posix()}) · [Лимиты и возврат остатка](../{paid_path.as_posix()})."])
        market_file=Path("tests/data/first_run_dex_2026-11-26/bancor_v3_ethereum.json.gz")
        wallet_file=Path("tests/data/first_run_dex_2026-11-26/bancor_v3_wallet.json.gz")
        with gzip.open(market_file,"rt",encoding="utf8") as stream:
            market=json.load(stream)
        with gzip.open(wallet_file,"rt",encoding="utf8") as stream:
            wallets=json.load(stream)
        active=sum(p["enabled"] for p in market["pools"].values())
        out.extend(["", f"Bancor V3 · Ethereum, блок {market['block']}. По реестру было {len(market['pools'])} пулов, "
                    f"торговля включена у {active}. Сверено {len(market['quotes'])} расчётов с NetworkInfo, "
                    f"{sum('spent' in c for c in market['cases'])} выплат на исходном контракте и {len(wallets['cases'])} через защищённый модуль или V5. "
                    "В dex.router указывается BancorV3Adapter с хешем из профиля. При создании нужны BancorNetwork, BancorNetworkInfo, MasterVault, WETH и обозначение ETH, в этом порядке. "
                    f"[Реестр и исходные обмены](../{market_file.as_posix()}) · [Выплаты и возврат остатка](../{wallet_file.as_posix()})."])
    classic_path = Path("tests/data/first_run_dex_2026-11-26/rubicon_classic_base.json.gz")
    if base and classic_path.exists():
        with gzip.open(classic_path, "rt", encoding="utf8") as stream:
            classic = json.load(stream)
        controls = classic["controls"]
        out.extend(["", f"Rubicon Classic, Base · на блоке 62 262 756 прочитано {classic['read_offers']} ID, "
            f"открыты {len(classic['active'])} заявки. В изолированном повторе получены {len(controls)} выплат. "
            "Квотер сверяет выбранные ID по исходной книге и вызывает calculateFees для каждой покупки. "
            "Кошелёк проверяет фактическое списание, чистое зачисление и срок. Для исполнения в dex.router "
            "указывается развёрнутый RubiconClassicWallet с указанным в профиле хешем. "
            f"[Ордера, выплаты и ограничения](../{classic_path.as_posix()})."])
    rubicon_base = Path("tests/data/first_run_dex_2026-11-26/rubicon_clmm_base.json")
    if base and rubicon_base.exists():
        registry = read(rubicon_base)
        out.extend(["", f"Rubicon CLMM · из {registry['registry_pairs']} пар публичного каталога Rubicon "
                    f"на четырёх проверенных уровнях комиссии найдены {len(registry['pools'])} пулов Base. "
                    "Для каждого подтверждён адрес CREATE2. Через V5 на трёх пулах прошли шесть полных "
                    "кругов, по два разных объёма на пул. Это охват рынка из реестра tickers по этим ставкам. "
                    f"[Реестр, тики, балансы и контроль minOut](../{rubicon_base.as_posix()})."])
    rubicon = Path("tests/data/first_run_dex_2026-11-26/rubicon_clmm_ethereum.json")
    if not base and rubicon.exists():
        rubicon_registry = read(rubicon)
        active = sum(int(p["liquidity"]) > 0 for p in rubicon_registry["pools"])
        out.extend(["", f"Rubicon CLMM · на Ethereum зафиксированы {len(rubicon_registry['pools'])} пулов и "
                    f"{len(rubicon_registry['tiers'])} уровня комиссии. Адрес каждого пула воспроизводится из фабрики, "
                    "пары, комиссии и байткода создания (CREATE2). На блоке 26 494 988 текущая активная "
                    f"ликвидность положительна у {active} пула. Два размера на нём прошли полный круг "
                    "через V5 с проверкой конечных балансов и minOut. "
                    f"[Адреса, события, тики и происхождение пулов](../{rubicon.as_posix()})."])
    if base:
        out.extend(["", "SquadSwap V3 · в двух проверенных парах Base ставка комиссии задана при создании пула. "
                    "Фабрика и TickLens совпадают с официальной таблицей. Указанный там v3InitCodeHash даёт иной адрес "
                    "для всех трёх зарегистрированных пулов WETH/USDC и WETH/USDT. Фактический код создания "
                    "и getPool независимо воспроизводят эти три адреса. Два пула провели восемь круглых обменов "
                    "с проверкой балансов через штатный V5; у третьего на этом блоке ликвидность равна нулю. "
                    "Для каждого направления учитывается фактически использованный вход. "
                    "[Архивные котировки и исполнения](../tests/data/first_run_dex_2026-11-26/squad_v3_base.json)."])
    if not base:
        out.extend(["", "CommunityV3 · фабрика Ethereum создала 434 пула. Все адреса проверены по CREATE2, getPool и nonce фабрики. "
                    "Подтверждены 25 ставок комиссии. На блоке 26 494 988 у 113 пулов ненулевая ликвидность на текущем тике. "
                    "На трёх рынках MUon/USDC и TSLAon/USDC выполнены девять полных кругов в V5, с исходным размером 0,01, 1 и 10 USDC. "
                    "[Реестр и результаты исполнения](../tests/data/first_run_dex_2026-11-26/community_v3_ethereum.json)."])
        depth_path = Path("tests/data/first_run_dex_2026-11-26/community_v3_depth.json")
        if depth_path.exists():
            depth = read(depth_path)
            markets = depth["markets"]
            rows = [q for m in markets for q in m["quotes"]]
            def complete(q):
                return int(q.get("out", 0)) > 0 and q["used"] == q["input"]
            full = sum(complete(q) for q in rows)
            partial = sum(int(q.get("out", 0)) > 0 and not complete(q) for q in rows)
            zero = sum(q.get("out") == "0" for q in rows)
            reverts = sum("error" in q for q in rows)
            sides = [len({q["side"] for q in m["quotes"] if complete(q)}) for m in markets]
            out.extend(["", f"На том же блоке VirtualQuoter проверил все {len(rows)} отдельных заявок: "
                        f"по 1 и 10 входных токенов в каждой стороне всех {len(markets)} пулов. "
                        f"Положительный выход на всём входе вернули {full} котировки, на частичном входе — {partial}. "
                        f"Нулевой выход получен в {zero} случаях, откат — в {reverts}. Полная котировка хотя бы на одном "
                        f"из двух размеров появилась в обеих сторонах у {sides.count(2)} пулов, только в одной у "
                        f"{sides.count(1)}, ни в одной у {sides.count(0)}. Денежная проверка V5 выше охватывает три пула. "
                        f"[Суммы входа и выхода по адресу пула](../{depth_path.as_posix()}) · "
                        "[Архивный прогон](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45887965733)."])
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
                    f"эти привязки по одному каноническому хешу при чтении и перед отправкой. dex.plugin_pool_offset = 75, dex.plugin_runtime_hashes = [`{qs['proxy_template_hash']}`]. "
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
        early_path=Path("tests/data/first_run_dex_2026-11-26/early_slipstream_wallets.json.gz")
        early=json.loads(gzip.decompress(early_path.read_bytes()))["cases"]
        partial=[x for x in early if int(x["spent"])<int(x["amount"])]
        out.extend(["", f"Ранняя фабрика Aerodrome Slipstream. На двух рынках WETH/BRETT и WETH/TOSHI "
                    f"получено {len(early)} архивных выплат через ArbExecutor и Universal Router, "
                    f"из них {len(partial)} частичных. При продаже 1 или 100 TOSHI списано "
                    f"{partial[0]['spent']} raw TOSHI и получено {partial[0]['received']} wei WETH. "
                    "Остаток входного токена сохраняется у плательщика. "
                    f"[Суммы, разрешения, газ и события](../{early_path.as_posix()})."])
    ring_path = Path("tests/data/first_run_dex_2026-11-26/ring_ethereum.json")
    if not base and ring_path.exists():
        ring = read(ring_path)
        directory_path = ring_path.with_name("ring_directory.json")
        directory = read(directory_path)
        proved = [r for r in directory["rows"] if r["verified"]]
        funded = sum(all(map(int, r["reserves"])) and all(int(directory["tokens"][w]["backing"] or 0) > 0
                     for w in r["wrappers"]) for r in proved)
        markets_path = ring_path.with_name("ring_markets.json")
        markets = read(markets_path)["markets"]
        successes = [sum(c["status"] == "both_swaps" for c in m["cases"]) for m in markets]
        cases = [c for pool in ring["pools"] for c in pool["cases"]]
        gas = [n for c in cases for n in c["gas"]]
        out.extend(["", f"**Ring Swap / Few, Ethereum.** На блоке {ring['block']} фабрика зарегистрировала {directory['count']} пары. "
                    f"Для {len(ring['pools'])} книг WETH с USDT, USDC, WBTC и UNI получены {2*len(cases)} обмена "
                    "через Universal Router: 0,001 и 0,01 WETH, обе стороны, оплата или получение в WETH и ETH. "
                    f"Газ одного обмена составил {min(gas)}–{max(gas)} единиц до отдельных разрешений. "
                    "Пара держит Few wrappers. Шаг роутера обменивает их с комиссией 0,3% и выдаёт исходную монету. "
                    "Для входящего USDT команда WRAP_FEW_TOKEN использует имеющийся allowance. В прямом "
                    "FEW_V2_SWAP_EXACT_IN попытка повторного approve откатилась. Нижняя граница выплаты проверена при "
                    "обоих направлениях. "
                    f"Для всех {directory['count']} пар проверены allPairs и обе стороны. Код пула, код обеих обёрток, "
                    f"CREATE2 и встречные записи обеих фабрик совпали у {len(proved)} пар; из них {funded} имели "
                    "ненулевые резервы и некоторое обеспечение обоих underlying. Объём, доступный для погашения, "
                    "проверяется вместе с ценой на одном блоке. В расширенной проверке 194 сценария начинались с ETH: "
                    f"{sum(successes)} завершили оба обмена, {sum(x>0 for x in successes)} рынков прошли хотя бы один "
                    f"размер, {sum(x==2 for x in successes)} — оба. В 65 проверках не найдено финансирование с "
                    "положительным округлённым выходом. Из 14 откатов 8 пришлись на выдачу сверх обеспечения, 2 — "
                    "на такую выдачу в отдельной паре пополнения, 4 — на отказ перевода при достаточном номинальном балансе. "
                    "У пула 132 меньший размер не дал целой единицы, больший обменял 2 минимальных единицы на 1. "
                    f"[Результат по каждому объёму](../{markets_path.as_posix()}) · "
                    "[Архив рынка](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45883763601). "
                    "Оператор может обнаружить пару по двум "
                    "адресам исходных активов; исполнитель допускает её после обычной проверки газа, налогов и выплаты. "
                    f"[Полный реестр](../{directory_path.as_posix()}) · "
                    "[Размещения](https://ringprotocol.gitbook.io/ring/docs/deployments) · "
                    f"[Суммы и результаты](../{ring_path.as_posix()}) · "
                    "[Архивный прогон](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45881656010)."])
    if not base:
        sn=read(Path("tests/data/first_run_dex_2026-11-26/supernova_ethereum_directory.json"))
        with gzip.open(Path("tests/data/first_run_dex_2026-11-26/supernova_ethereum_wallets.json.gz"),"rt",encoding="utf8") as stream:
            swaps=json.load(stream)["markets"]
        out.append(f"**Supernova CL.** На блоке {sn['block']} подтверждены {len(sn['markets'])} пулов "
                   f"четырёх реестров customDeployer. Адаптер проверяет CREATE2, код плагина и комиссию "
                   "с учётом скидки плательщика. Внешний вызов Reflex может изменить другой рынок, "
                   "поэтому одиночный пул сравнивается с обычным маршрутом отдельно. "
                   f"V5 подтвердил {sum(len(m['wallet']) for m in swaps)} полных обменов пяти пар на двух размерах "
                   "в обе стороны и столько же откатов при повышении минимума на единицу. "
                   "Для отдельного адреса с зарегистрированной скидкой проверены две дополнительные выплаты WETH/USDT. Расчёт действующей сделки использует скидку её плательщика. "
                   "[Выплаты кошелька](../tests/data/first_run_dex_2026-11-26/supernova_ethereum_wallets.json.gz). "
                   "[Пулы и источники комиссии](../tests/data/first_run_dex_2026-11-26/supernova_ethereum_directory.json).")
        out.append("**Supernova AMM.** На том же блоке фабрика зарегистрировала 14 пар. Отдельный PairGenerator создал 12 волатильных и две стабильные пары. "
                   "На одном пуле каждого типа V5 подтвердил восемь выплат на двух размерах в обоих направлениях. Минимум на единицу выше результата вызвал восемь откатов. "
                   "Ставка читается для конкретного пула через factory.getFee(pool,stable). "
                   "[Реестр и ставки](../tests/data/first_run_dex_2026-11-26/ethereum_classic_registry_supernova-amm.json.gz) · "
                   "[Суммы и остатки](../tests/data/first_run_dex_2026-11-26/ethereum_classic_wallets_supernova-amm.json.gz).")
        solidly_directory=json.loads(gzip.decompress(Path("tests/data/first_run_dex_2026-11-26/solidly_v2_ethereum.json.gz").read_bytes()))
        out.append(f"**Solidly V2 Ethereum.** На блоке {solidly_directory['height']} фабрика подтвердила {len(solidly_directory['markets'])} пар через CREATE2 и оба направления getPair. "
                   "У 66 пар ненулевые резервы совпали с обоими балансами. У одной пары резервы нулевые, у USDC/DEI фактический баланс DEI обнулился. "
                   "Шаг Solidly в ArbExecutor V5 использует getAmountOut самой пары и проверяет минимум выплаты. У обеих кривых комиссия — feeRatio пары в миллионных. "
                   "Для фабрики проверяется logicAddress, для каждой пары — её фабрика и childSubImplementationAddress; implementationAddress служит обозначением интерфейса. "
                   "Результаты кошелька на двух контрольных парах приведены для V5 и официального Router01. У Router01 минимум номинальный. Прямой маршрут доступен при проверенном неизменяемом коде входного и выходного ERC-20 (WETH, WBTC, DAI, LUSD); фактический кошелёк проверяется перед отправкой и по квитанции. Для USDT и USDC применяется V5. Управляющий адрес Router01 может обновлять его логику после проверки состояния перед отправкой. "
                   "[Реестр фабрики](../tests/data/first_run_dex_2026-11-26/solidly_v2_ethereum.json.gz) · [Обмены и откаты](../tests/data/first_run_dex_2026-11-26/solidly_v2_ethereum_wallets.json.gz).")
        integral_path=Path("tests/data/first_run_dex_2026-11-26/integral_size_ethereum.json.gz")
        integral=json.loads(gzip.decompress(integral_path.read_bytes()))
        ih=integral["historical"]
        out.append(f"**Integral SIZE.** На блоке {integral['height']} официальная фабрика вернула {len(integral['markets'])} пар. "
                   f"Из них релеер допускал к обмену {sum(m['atomic_enabled'] for m in integral['markets'])}. Официальные quoteSell и quoteBuy в обоих направлениях "
                   f"вернули TR5A во всех {len(integral['atomic_checks'])} вызовах. Токены оставались на обоих балансах {sum(all(int(n)>0 for n in m['pool_balances']) for m in integral['markets'])} пар. "
                   f"На блоке {ih['block']} релеер выплатил получателю 39 186,890298 USDT за 16,168011965784978830 WETH в исходной транзакции. "
                   f"Связанную заявку релеера №{ih['order']} контракт Delay исполнил через {ih['lag_seconds']} с. Собственная заявка через Delay блокирует входные средства до отдельного исполнения. "
                   "На каждой новой канонической голове бот проверяет isPairEnabled у зарегистрированных пар, включая рынки с пустой лесенкой. После включения он проверяет роли, аванс ETH релеера и фактические уровни обмена. "
                   "Для возврата мгновенного пути нужно включение пары, достаточный запас выходной валюты и ETH у релеера, "
                   "исполнение для суммы и подтверждение чистой выплаты получателю. [Реестр, балансы и квитанции](../tests/data/first_run_dex_2026-11-26/integral_size_ethereum.json.gz).")
        integral_wallet_path=integral_path.with_name("integral_size_ethereum_wallets.json.gz")
        iw=json.loads(gzip.decompress(integral_wallet_path.read_bytes()))
        ic=iw["historical"]["cases"]
        queue=iw["current"]["queued"]
        out.append(f"На более раннем блоке {iw['historical']['height']} восемь архивных обменов WETH/USDT подтвердили списания и выплаты кошелька при точном входе и точном выходе. "
                   f"Они потребовали {min(c['gas'] for c in ic):,}–{max(c['gas'] for c in ic):,} единиц газа. Для каждого обмена неверный предел на одну минимальную единицу и истёкшая заявка дали откат. "
                   f"На блоке {integral['height']} публичная заявка Delay №{queue['order']} в симуляции перевела в контракт 1 000 USDT и аванс 0,0001539332645 ETH. Кошелёк получил 0 WETH в этой транзакции. "
                   "[Параметры транзакций, квитанции и остатки](../tests/data/first_run_dex_2026-11-26/integral_size_ethereum_wallets.json.gz).")
        hello_dir_path=Path("tests/data/first_run_dex_2026-11-26/hello_ethereum_directory.json")
        hello=read(hello_dir_path)
        hello_logs_path=hello_dir_path.with_name("hello_ethereum_swaps.json")
        hello_logs=read(hello_logs_path)
        usdt="0xdac17f958d2ee523a2206206994597c13d831ec7"
        cash=sum(int(hello_logs["markets"][m["pool"]]["volume_raw"][m["tokens"].index(usdt)]) for m in hello["markets"] if usdt in m["tokens"] and m["pool"] in hello_logs["markets"])
        out.append(f"**HELLO V3.** На блоке {hello['height']} база содержит {len(hello['markets'])} зарегистрированных пулов. У {sum(int(m['liquidity'])>0 for m in hello['markets'])} была ликвидность на текущем тике. "
                   f"За последние 24 часа {hello_logs['swap_count']} событий Swap пришли из {len(hello_logs['markets'])} пулов. Оборот по двум сторонам пар с USDT составил {cash//10**6}.{cash%10**6:06d} USDT. "
                   f"Отдельно в этих транзакциях зафиксировано {len(hello_logs['protocol_transfers'])} переводов накопленной комиссии получателю протокола. "
                   f"[Реестр и CREATE2](../{hello_dir_path.as_posix()}) · [События, получатель комиссии и квотеры](../{hello_logs_path.as_posix()}).")
        hello_wallet_path=hello_dir_path.with_name("hello_ethereum_wallets.json.gz")
        with gzip.open(hello_wallet_path,"rt",encoding="utf8") as stream:
            hello_wallet=json.load(stream)
        legs=sum(2*len(m["cases"]) for m in hello_wallet["settlements"])
        cash_case=next(m for m in hello_wallet["settlements"] if m["cash"]==usdt)
        costs={c["mode"]:[c[k]["gas"] for k in ("buy","sell")] for c in cash_case["cases"] if int(c["buy"]["input"])==10000}
        out.append(f"На двух рынках HELLO кошельки прошли {legs} обмена через Router, Router02 и V5 с одинаковыми выплатами. Каждый повышенный на одну единицу минимум и каждый истекший срок вызвали откат. "
                   f"При входе 0.01 USDT покупка и обратная продажа вернули 0.009940 USDT до газа. На эти две операции ушло {costs['router'][0]}/{costs['router'][1]} единиц газа у Router, "
                   f"{costs['router02'][0]}/{costs['router02'][1]} у Router02 и {costs['v5'][0]}/{costs['v5'][1]} у V5. Расходы на необходимое разрешение ERC20 учитываются отдельно. "
                   f"V5 проверяет изменение конечного баланса. Официальный Router сравнивает минимум с номинальным выходом пула. [Вызовы и остатки кошельков](../{hello_wallet_path.as_posix()}).")
        temple_registry_path=Path("tests/data/first_run_dex_2026-11-26/temple_registry_ethereum.json")
        tr=read(temple_registry_path)
        temple_wallet_path=temple_registry_path.with_name("temple_ethereum_wallets.json.gz")
        with gzip.open(temple_wallet_path,"rt",encoding="utf8") as stream:
            tw=json.load(stream)
        gas_by_mode={case["mode"]:[int(case[leg]["gas"],16) for leg in ["buy","sell"]] for case in tw["cases"]}
        out.append(f"**TempleDAO.** Официальный роутер принимает FRAX для покупки TEMPLE. На продаже TEMPLE он сравнивает две округлённые выплаты. "
                   f"При равенстве платит пул в FRAX. Когда выше ставка казначейства, роутер сжигает TEMPLE и платит DAI со своего баланса. "
                   f"На блоке {tr['height']} остаток DAI равен {tr['defense_balance']}. Размер 1 минимальная единица TEMPLE котируется в 2 минимальные единицы FRAX, а 10 000 TEMPLE требуют 8 000 DAI. "
                   f"В изолированном исполнении покупки на 1 и 10 FRAX и продажи купленного TEMPLE дали одинаковые выплаты тремя способами. "
                   f"Газ покупки/продажи составил {gas_by_mode['router'][0]}/{gas_by_mode['router'][1]} у официального роутера, "
                   f"{gas_by_mode['protected'][0]}/{gas_by_mode['protected'][1]} у защитного модуля и {gas_by_mode['v5'][0]}/{gas_by_mode['v5'][1]} у V5. Учтён только газ самого обмена. "
                   f"Разрешение ERC20 оценивается отдельно по расходам плательщика. Владелец задаёт адрес развёрнутого модуля пары для двух путей: прямой вызов кошелька или шаг V5. Оба фиксируют токен и чистый минимум в транзакции. "
                   f"Официальная покупка возвращает ноль в ABI и выбирает пару из изменяемого реестра. На этом блоке изменение требует {tr['role_controls'][tr['owners'][1]]['threshold']} из {tr['role_controls'][tr['owners'][1]]['members']} подписей. "
                   f"[Реестр, резервы и порог](../{temple_registry_path.as_posix()}) · [Кошелёк, газ, минимум и срок](../{temple_wallet_path.as_posix()}).")
        carbon_path=Path("tests/data/first_run_dex_2026-11-26/carbon_ethereum.json.gz")
        with gzip.open(carbon_path,"rt",encoding="utf8") as stream:
            carbon=json.load(stream)
        registry_path=carbon_path.with_name("carbon_registry_ethereum.json")
        carbon_events=read(registry_path)
        carbon_pairs=carbon["directory"]
        usdt="0xdac17f958d2ee523a2206206994597c13d831ec7"
        stable_raw=sum(int(row["amount"+str(row["tokens"].index(usdt))]) for row in carbon_events["markets"] if usdt in row["tokens"])
        out.append(f"**Carbon DeFi Ethereum.** Контроллер версии 6 зарегистрировал {len(carbon_pairs)} пар с {sum(p['count'] for p in carbon_pairs)} стратегиями. "
                   f"У {sum(p['count']==0 for p in carbon_pairs)} пар на блоке снимка не осталось стратегий. USDC/ETH и USDC/WETH имеют разные книги и разные идентификаторы, 5 и 602. "
                   f"За блоки {carbon_events['event_window'][0]}–{carbon_events['event_window'][1]} контроллер зарегистрировал {sum(m['swaps'] for m in carbon_events['markets'])} обменов в {carbon_events['trade_txs']} транзакциях на {len(carbon_events['markets'])} парах. Учитывая только сторону USDT, по журналу прошло {stable_raw/1e6:.6f} USDT. "
                   f"Архивное исполнение подтвердило {len(carbon['controls'])} выплат кошельку в USDC↔ETH и USDT→ETH с откатами при более строгом пределе и истёкшем сроке. "
                   "В 16 контрольных задачах 35 вариантов оплачены на одном состоянии кошелька. При цене газа блока лучший до газа вариант сменился в 5 случаях, при добавлении 1 gwei за единицу газа — в 8. В стоимость включён требуемый approve. [Котировки, выплаты и сравнение по газу](../tests/data/first_run_dex_2026-11-26/carbon_allocations_ethereum.json.gz). "
                   "Для прямого выхода в ETH проверяются контроллер, операции кошелька и выплата по TokensTraded. Для выхода в ERC20 требуется указанный оператором CarbonWallet с проверенными runtime и контроллером. Он возвращает неиспользованный аванс и проверяет приток в контроллер и чистое поступление отправителю в той же транзакции. В 12 архивных исполнениях модуль затратил 170138–188136 единиц газа на вызов обмена. [Суммы и газ модуля](../tests/data/first_run_dex_2026-11-26/carbon_ethereum_protected.json.gz). "
                   f"[Реестр, комиссия и события](../{registry_path.as_posix()}) · [Стратегии и движения кошелька](../{carbon_path.as_posix()}).")
        ep=Path("tests/data/first_run_dex_2026-11-26/ekubo_v2_ethereum_activity.json.gz")
        with gzip.open(ep,"rt",encoding="utf8") as stream:
            ev=json.load(stream)
        wallets_path=ep.with_name("ekubo_v2_ethereum_wallets.json.gz")
        with gzip.open(wallets_path,"rt",encoding="utf8") as stream:
            wallets=json.load(stream)["cases"]
        filled=[r for r in wallets if r["exact"]["result"]["status"]=="0x1"]
        gas=[int(r["exact"]["result"]["gasUsed"],16) for r in filled]
        trades=ev["swaps"]; stable_id=next(m["id"] for m in ev["markets"] if m["key"]["token1"]=="0xdac17f958d2ee523a2206206994597c13d831ec7")
        stable=[s for s in trades if s["id"]==stable_id]
        out.append(f"**Ekubo V2 Ethereum.** С блока {ev['window'][0]['number']} по {ev['window'][1]['number']} Core V2 выпустил {len(trades)} событий обмена в {len(ev['markets'])} пулах. "
                   f"В USDC/USDT зарегистрировано {len(stable)} обменов. Оборот одной стороны составил {sum(abs(int(s['delta1'])) for s in stable)/1e6:,.6f} USDT. "
                   "На последнем блоке окна две заявки USDT→USDC получили частичную котировку с расходом 0,023556 USDT. Обе откатились при требовании полного входа. "
                   f"На двух меньших размерах это направление исполнилось. На пяти рынках кошелёк получил {len(filled)} полных выплат с расходом газа от {min(gas):,} до {max(gas):,} единиц. Повышение минимума любой выплаты на одну минимальную единицу вызвало откат без списания. "
                   f"[Выплаты и откаты](../{wallets_path.as_posix()}) · "
                   f"[События, ключи и котировки](../{ep.as_posix()}) · [Контракты V2](https://docs.ekubo.org/reference/contracts/evm-v2/).")
        dx=read(Path("tests/data/first_run_dex_2026-11-26/deltaswap_ethereum.json"))["current"]
        x7=read(Path("tests/data/first_run_dex_2026-11-26/ethereum_v2_factories.json"))
        for key in ("x7-finance","x7-finance-ethereum"):
            row=x7[key]; sample=next(x for x in row["samples"] if "cases" in x)
            out.append(f"**{key}.** Фабрика `{row['factory']}` подтвердила {len(row['pools'])} пар. Обычный V5 swap в "
                       f"`{sample['pool']}` подтвердил расход и выплату в двух направлениях на двух объёмах. "
                       f"Ставка этого entrypoint составляет 0,2%. Для маршрута со скидкой через отдельный роутер нужна проверка ставки самого плательщика. "
                       f"Из проверенных перед этим пулом кандидатов {sum('reason' in z for z in row['samples'])} не прошли по состоянию балансов или результату обмена. "
                       "[Реестр, исходники и обмены](../tests/data/first_run_dex_2026-11-26/ethereum_v2_factories.json).")
        checks=sum(len(t.get("trades",[])) for p in dx["other_wallets"] for t in p["samples"])+sum(len(t) for t in dx["wallet"])
        out.append(f"**DeltaSwap Ethereum.** В реестре {len(dx['pools'])} пар. Официальный Router02 подтвердил {checks} обменов "
                   "по обеим сторонам трёх рынков с точным списанием и порогом на 1 raw выше выплаты. "
                   "На этом блоке параметры фабрики равны [3, 0], комиссия обычного кошелька составляет 0,3%. "
                   "[Реестр и денежные результаты](../tests/data/first_run_dex_2026-11-26/deltaswap_ethereum.json).")
        nine=x7["9inch-ethereum"]
        out.append(f"**9inch Ethereum.** Фабрика подтвердила {len(nine['pools'])} CREATE2-пар. "
                   "Ставка стандартного swap составляет 0,29%. В двух направлениях на двух размерах "
                   "V5 получил расчётный выход, а повышение минимума на единицу вызвало откат. "
                   "[Реестр, код и предел выплаты](../tests/data/first_run_dex_2026-11-26/ethereum_v2_factories.json).")
        out.append("")
    mint_path=Path(f"tests/data/first_run_dex_2026-11-26/mint_club_{network}_wallets.json.gz")
    with gzip.open(mint_path,"rt",encoding="utf8") as stream:
        mint=json.load(stream)
    reserves=len({row["reserve"] for row in mint["directory"]})
    registry=read(Path(f"tests/data/first_run_dex_2026-11-26/mint_club_{network}_events.json"))
    reserve_count=len({t["reserve"] for t in mint["directory"]})
    pairs=mint["markets"]
    trials=sum(len(p["trials"]) for p in pairs)
    out.append(f"**Mint Club V2.** Bond зарегистрировал {mint['total']} выпусков ERC20 и ERC1155 с {reserves} видами обеспечения. "
               f"На блоке {mint['height']} три проверенных ERC20 с резервом WETH прошли {trials} циклов выпуска и погашения. "
               "Уменьшение максимальной суммы покупки и увеличение минимальной суммы погашения на одну минимальную единицу дали откат. "
               f"Защитный модуль принимает собственные средства подписанта. В архиве дополнительно проверены два выпуска и два погашения в расчётной валюте {'USDC' if network=='base' else 'USDT'}, с проверкой исходного и конечного остатка. "
               "Для отслеживаемого токена проверяются tokenBond, байткод клона и его реализации, ступени цены и текущий выпуск. "
               "Для погашения используются остаток собственного обеспечения рынка и фактический баланс резервного ERC20 общего Bond. "
               f"С блока {registry['window']['from']} по {registry['window']['to']} Bond выпустил {sum(registry['window']['counts'])} событий Mint и Burn. "
               f"[Кошелёк и газ](../{mint_path.as_posix()}) · [Регистрация и код](../tests/data/first_run_dex_2026-11-26/mint_club_{network}_events.json).")
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
        if base and id in ("umbrae-damm","umbrae-dlmm"):
            version=id.split("-")[1]
            markets=ud["markets"][version]
            if not pool: pool=markets[0]["pool"]
            if any(m["pool"]==pool.lower() for m in markets):
                factory=ud["roles"][version+"Factory"]["address"]
                roundtrip="getPair(tokenX,tokenY,binStep,baseFactor) и beacon" if version=="dlmm" else "getPair(tokenX,tokenY) и beacon"
        if id in open_pages and sample.get("samples"):
            selected=sample["samples"][0]
            pool,factory=selected["address"],selected["reported_factory"]
            if id in {"x7-finance","x7-finance-ethereum","deltaswap-ethereum","9inch-ethereum"}:
                roundtrip="getPair(address,address)"
            elif id=="supernova-cl" and any(m["pool"]==pool.lower() and m["registered"] for m in sn["markets"]):
                roundtrip="customPoolByPair(customDeployer,token0,token1)"
            elif id=="solidlydex" and any(m["pool"]==pool.lower() for m in solidly_directory["markets"]):
                factory,roundtrip=solidly_directory["factory"],"getPair(a,b,stable) × 2 и CREATE2"
            elif id=="supernova-amm":roundtrip="getPair(token0,token1,stable)"
        if base and id == "quickswap-v2-base" and quickswap_v2.exists() and pool and any(m["pool"] == pool.lower() for m in q2["directory"]):
            factory, roundtrip = q2["factory"], "getPair(address,address)"
        if base and id == "quickswap-v4-base" and quickswap.exists() and pool and any(m["pool"] == pool.lower() for m in qs["markets"]):
            factory, roundtrip = qs["addresses"]["AlgebraFactory"], "poolByPair(address,address)"
        if not base and id=="antfarm-ethereum" and pool:
            with gzip.open("tests/data/first_run_dex_2026-11-26/antfarm_ethereum_registry.json.gz","rt",encoding="utf8") as archive:
                antfarm=json.load(archive)
            if any(m["pair"]==pool.lower() for m in antfarm["pairs"]):
                factory=antfarm["contracts"]["AntfarmFactory"]["address"]
                roundtrip="CREATE2 и getPair(token0,token1,fee) в обоих порядках"
        if not base and id=="hello-dex-ethereum" and pool and any(m["pool"]==pool.lower() for m in hello["markets"]):
            factory,roundtrip=hello["factory"],"getPool(token0,token1,fee) в обе стороны и CREATE2 от отдельного deployer"
        if not base and id=="concave" and id in configured and pool and any(m["pool"]==pool.lower() for m in cv["markets"]):
            factory,roundtrip=configured[id]["settings"]["dex.factory"],"клон типа 1 → CREATE2 → getPair в обоих порядках"
        if not base and id=="bitswap" and id in configured and pool and any(m["pool"]==pool.lower() for m in bs["markets"]):
            factory,roundtrip=bs["factory"],"NewPool, CREATE2 и pool(x,y,fee) в обоих порядках"
        if not base and id=="templedao" and pool:
            tr=read(Path("tests/data/first_run_dex_2026-11-26/temple_registry_ethereum.json"))
            if pool.lower()==tr["pair"]:
                factory,roundtrip=tr["router"],"tokenPair(FRAX) → пара, pair.router() → роутер"
        if not base and id=="carbon-defi-ethereum" and pool and pool.split("-")[0].lower()==carbon["roles"][0]:
            factory=carbon["roles"][0]
            roundtrip="адрес контроллера совпал. Суффикс индекса не задаёт ID пары в Controller.pair"
        if not base and id == "ekubo-v2-ethereum" and pool and pool.lower().split("-")[-1] in {m["id"] for m in ev["markets"]}:
            factory,roundtrip=ev["core"]["address"],"PoolInitialized → keccak(ключ) → Core.sload"
        if not base and id == "swapr_ethereum":
            factory,roundtrip = "0xd34971bab6e5e356fd250715f5de0492bb070452","getPair(address,address)"
        if base and id == "hydrex-integral" and Path(hydrex).exists():
            hx = read(Path(hydrex))
            if any(r["pool"] == pool.lower() and r["listed"] for r in hx["directory"]):
                factory, roundtrip = hx["addresses"]["Factory"], "poolByPair(address,address)"
        if id in matched:
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
        if not base and id in ("cypher","degenswap","baseline-ethereum") and "degenswap-ethereum" in configured:
            if id=="degenswap":
                pool=next(p["pool"] for p in degen["markets"] if all(int(v or 0)>0 for v in p["balances"]+p["reserves"]))
                factory=degen["factory"]["address"]; roundtrip="allPairs → CREATE2 → getPair в обоих порядках"
                status="профиль degenswap-ethereum; требуется проверенное размещение модуля владельца"
            elif id=="cypher":
                cypher_v4_public=json.loads(gzip.decompress((ds/"cypher_v4_ethereum_directory.json.gz").read_bytes()))
                pool=cypher_v4_public["markets"][0]["pool"];factory=cypher_v4_public["factory"]["address"]
                roundtrip="19 из 19 строк GeckoTerminal относятся к пулам фабрики Cypher V4"
                status="dex.indexer_id = cypher у профиля cypher-v4-ethereum. Cypher V2 использует свой собственный реестр фабрики"
            else:
                keyset=json.loads(gzip.decompress((ds/"baseline_ethereum_directory.json.gz").read_bytes()))
                pool=baseline["markets"][0]["pool"]; factory=keyset["relay"];roundtrip="PoolId ↔ bToken в Relay, Initialize в PoolManager"
                status=f"профиль Baseline Ethereum; {deals} архивных выплат"
        if not base and id == "integral_size":
            market=next(m for m in integral["markets"] if set(m["tokens"])=={ih["input"],ih["output"]})
            pool,factory,roundtrip=market["pair"],integral["roles"]["factory"]["address"],"allPairs + getPair в обоих направлениях"
            status="профиль integral-size-ethereum, dex.indexer_id = integral_size; на контрольном блоке релеер отключил все 15 пар. Повторная проверка выполняется при новом блоке"
        if not base and id == "ring-exchange-ethereum" and ring_path.exists():
            status = "Ring Few · адресная проверка всех пар; котировки, обеспечение и контрольные обмены выше"
        if not base and id=="toonswap" and id in configured:
            pool=pool or toon["markets"][0]["pool"]
            if any(m["pool"]==pool.lower() for m in toon["markets"]):
                factory=toon["roles"]["factory"]
                roundtrip="CREATE2 + getPair в обоих порядках"
        if not base and id=="sashimiswap" and id in configured:
            with gzip.open("tests/data/first_run_dex_2026-11-26/sashimi_ethereum_registry.json.gz","rt",encoding="utf8") as stream:
                market_registry=json.load(stream)
            pool=pool or next(r["pool"] for r in market_registry["pools"] if all(map(int,r["reserves"])))
            if any(r["pool"]==pool.lower() for r in market_registry["pools"]):
                factory=configured[id]["settings"]["dex.factory"]
                roundtrip="CREATE2, реестр пар и учёт обоих токенов у Router02"
        if not base and id=="safemoonswap_ethereum":
            dep=safeswap["deployments"][0]
            factory=dep["roles"]["factory"]["address"]
            pool=next(m["pool"] for m in dep["markets"] if m.get("rounds"))
            roundtrip="CREATE2 и getPair в обоих порядках"
            status="Четыре фабрики с архивными выплатами. В первом запуске две фабрики с Router02 и V5, а также proxy-1 с исполнением V5 непосредственно в неизменяемой паре. Для proxy-2 подтверждены выплаты через SafeSwapTradeRouter и модуль кошелька с оплатой из WETH"
        if not base and id=="surge-protocol-eth":
            pool=surge["address"]
            roundtrip="проверенный код SURGE и собственные события Bought и Sold"
            status="Проверены ETH/SRG, защищённый WETH/SRG и две стороны пути V5 USDT/WETH/SRG. Для других контрактов SRG20 нужен отдельный реестр"
        if not base and id in ("bundles-swap","dagrow"):
            stem="bundles" if id=="bundles-swap" else "dagrow"
            with gzip.open(f"tests/data/first_run_dex_2026-11-26/{stem}_ethereum_directory.json.gz","rt") as handle:
                observed=json.load(handle)
            if id=="bundles-swap":
                factory=observed["factory"]
                pool=observed["router"]["oracle_pair"]
                roundtrip="Все 11 пар прошли CREATE2 и getPair в обоих порядках. Роутер подтверждён событием Swap"
                status="1 OraclePair и 10 Pair. В обычных парах дополнительно оплачивается комиссия в BUN. Покрытие исполнения проверяется отдельно"
            else:
                pool=observed["roles"]["pool"]["address"]
                factory=None
                roundtrip="Один пул USDT/DAGR. Проверены proxy, implementation и связанный poolGuard"
                status="8 котировок Eco/Exo и 20 результатов в архивной EVM: оба направления, две суммы, SSP и модуль в двух формах заявки, V5 kind 29. Адрес модуля задаёт владелец после развёртывания"
        if not base and id=="ranges-finance" and id in configured:
            ranges=json.loads(gzip.decompress(Path("tests/data/first_run_dex_2026-11-26/ranges_ethereum_directory.json.gz").read_bytes()))
            factory=ranges["roles"]["factory"]
            pool=pool or ranges["pools"][0]["pool"]
            roundtrip="getPools, isPoolFromFactory, регистрация Vault и getHook"
            status=f"Puzzle Ranges · {len(ranges['pools'])} зарегистрированы, {sum(p['initialized'] for p in ranges['pools'])} инициализированы"
        specific=base_launch_inventory.registry_row(network,id)
        if specific:
            selected, factory, roundtrip, status = specific
            pool = selected or pool
        # Эти строки имеют собственное доказательство и уточнение допуска вместо общей подписи.
        if (id in configured and status != "профиль, см. параметры выше"
                and "профиль" not in status.lower()):
            status = f"профиль `{id}` · {status}"
        if not base and id == "cypher":
            assert configured["cypher-v4-ethereum"]["settings"]["dex.indexer_id"] == id
            status = "профиль `cypher-v4-ethereum` · " + status
        if base and id == "treble-v4-limit":
            assert "treble-v4" in configured
            status += " · базовый профиль `treble-v4`; расчёт пассивной заявки зависит от исполнения эпохи"
        if base and id == "ryze":
            status += " · персональный RFQ через настройки Ryze площадки Base; нужен доступ своего кошелька"
        if base and id in ("ensurer", "dejungle"):
            status += f" · `T-DEX-{network}-{id}` найти первичное размещение Base и привязать событие обмена к фабрике"
        if id in ("solidly-v3-base", "solidly-v3"):
            status += " · оператор задаёт собственный адрес принятого модуля"
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
    top_data = read(Path("tests/data/first_run_dex_2026-11-26/llama_top_bindings.json"))
    top = {item["id"]:item for item in top_data["networks"][network]["rows"]}
    data_dir = Path("tests/data/first_run_dex_2026-11-26")
    base_cl_archive = data_dir / "wagmi_thick_base.json.gz"
    community_wallets = json.loads(gzip.decompress((data_dir / "wagmi_thick_modules.json.gz").read_bytes())) if base else {}
    base_cl = json.loads(gzip.decompress(base_cl_archive.read_bytes())) if base else {}
    registry_sources = read(data_dir / "llama_factory_scopes.json")
    registry_entries = registry_sources["networks"][network]
    archive_v2 = data_dir / ("llama_base_v2_wallets.json.gz" if base else "llama_eth_v2_wallets.json.gz")
    legacy_v2 = json.loads(gzip.decompress(archive_v2.read_bytes()))
    hashflow_data = read(Path("tests/data/hashflow_direct_2026-11-26/markets_hour.json"))
    hashflow = next(n for n in hashflow_data["networks"] if n["chain"] == (8453 if base else 1))
    king_shark = {s["id"]:s for s in read(Path("tests/data/first_run_dex_2026-11-26/king_sharkswap_source_bindings.json"))
                  if s["network"] == network}
    for row in sorted(llama["rows"], key=lambda row: -volumes.get(row["id"], -1)):
        r = adapters[row["id"]]
        ref = r.get("link") or r.get("source")
        candidates = r.get("addresses_from_chain_stanza", [])
        task = f"`T-SOURCE-{network}-{row['id']}`"
        if source_scope := registry_entries.get(str(row["id"])):
            assert source_scope["module"] == row["module"] and source_scope["path"] == r["path"]
            start, end = source_scope["lines"]
            ref = ref.split("#")[0] + f"#L{start}-L{end}"
            candidates = [source_scope["factory"]] if "factory" in source_scope else []
            kind = source_scope["kind"]
            if candidates:
                same = [p for p in configured.values() if p["settings"].get("dex.factory", "").lower() == candidates[0]]
                if len(same) == 1:
                    task = f"Фабрика подтверждена в сетевой записи источника и в профиле `{same[0]['id']}`"
                    slug = same[0]["id"].removesuffix("-" + network)
                    if slug == "theorem" and network == "ethereum":
                        snapshot = data_dir / "theorem_ethereum.json.gz"
                        captured = json.loads(gzip.decompress(snapshot.read_bytes()))
                        directory, wallets = captured["directory"], captured["wallets"]
                        assert directory["roles"]["factory"]["address"] == candidates[0] and directory["selector"] == wallets["selector"]
                        active = sum(int(m["liquidity"]) > 0 for m in directory["markets"])
                        counts = {wire: sum("received" in c.get(side, {}) for c in wallets["cases"] if c["wire"] == wire
                                            for side in ("buy", "sell")) for wire in ("router", "router_fot", "router_sweep", "module", "v5")}
                        task += (f" · {len(directory['markets'])} зарегистрированных пулов, {active} с активной ликвидностью. "
                                 f"На двух размерах V5 подтвердил {counts['v5']} выплат с проверкой чистого минимума, "
                                 f"три штатных входа Router — по {counts['router']} каждый. На USDT→A7A5 каждый штатный вход "
                                 f"дважды принял минимум на атом выше зачисленного. Модуль AlgebraRouterWallet подтвердил "
                                 f"{counts['module']} чистых выплат, в том числе две продажи A7A5 с фактическим списанием "
                                 f"на атом меньше номинала. V5 на этих продажах откатился. На WETH/wA7A5 обычный Router "
                                 f"сэкономил 19 326–26 047 gas относительно V5 при одинаковом расходе и выплате. "
                                 f"Для одобренного владельцем модуля укажите dex.v3_wallet_module. "
                                 f"[Фабрика, плагины, квитанции и отказы](../{snapshot.as_posix()})")
                    if slug == "goblins" and base:
                        directory_path = data_dir / "goblins_base_directory.json.gz"
                        directory = json.loads(gzip.decompress(directory_path.read_bytes()))
                        cash_path = data_dir / "goblins_base_cash_wallets.json.gz"
                        native_path = data_dir / "goblins_base_wallets.json.gz"
                        cash, native = (json.loads(gzip.decompress(path.read_bytes())) for path in (cash_path, native_path))
                        assert directory["factory"] == candidates[0] and directory["selector"] == cash["selector"] == native["selector"]
                        count = len(cash["cases"]) + len(native["cases"])
                        active = sum(int(m["liquidity"]) > 0 for m in directory["pools"])
                        native_gas = {(c["pool"],c["input"],c["spent"]):c["gas"] for c in cash["cases"] if c["wire"]=="router02"}
                        saved = [c["gas"]-native_gas[(c["pool"],c["input"],c["spent"])] for c in cash["cases"] if c["wire"]=="v5"]
                        task += (f" · {len(directory['pools'])} пулов, {active} с ликвидностью на текущем тике, "
                                 f"число ставок по корню реестра — {len(directory['tiers'])}. [Доказательство](../{directory_path.as_posix()}). "
                                 f"{count} выплат на пяти пулах, на двух размерах в обе стороны, с проверкой минимума и срока. "
                                 f"Для DEGEN/AERO Router02 сэкономил {min(saved)}–{max(saved)} gas относительно V5. "
                                 f"Для двух обновляемых токенов в парах с WETH разрешён V5, а после размещения модуля также защищённый кошелёк "
                                 f"(`dex.v3_wallet_module`). Расход на подготовку DEGEN отдельный. "
                                 f"[WETH/USDC и WETH/токен](../{native_path.as_posix()}) · [DEGEN/AERO](../{cash_path.as_posix()})")
                    if slug in legacy_v2:
                        book = legacy_v2[slug]
                        sample = next(s for s in book["samples"] if "cases" in s)
                        assert book["factory"] == candidates[0] == sample["factory"]
                        task += (f" · {len(book['pools'])} зарегистрированных пар, покупка и обратная продажа "
                                 f"на {len(sample['cases'])} объёмах в [кассовом повторе](../{archive_v2.as_posix()})")
                    if slug in base_cl:
                        book = base_cl[slug]
                        assert book["roles"]["factory"]["address"] == candidates[0]
                        cases = book["cases"]
                        task += (f" · {len(book['markets'])} пулов по CREATE2 и реестру в обе стороны; "
                                 f"на {len({c['pool'] for c in cases})} пулах проверено {len(cases)} выплат "
                                 f"через Router02 и V5. [Блок, линза, суммы и квитанции](../{base_cl_archive.as_posix()})")
                        if slug in community_wallets:
                            wallet = community_wallets[slug]
                            assert same[0]["settings"]["dex.module_runtime_hashes"] == [wallet["module_hash"]]
                            storage_file = data_dir / (slug + "_base_storage.json.gz")
                            storage = json.loads(gzip.decompress(storage_file.read_bytes()))
                            assert set(same[0]["settings"]["dex.v3_fee_tiers"]) == set(storage["tiers"])
                            task += (f" · полный набор комиссий подтверждён [корнем хранилища](../{storage_file.as_posix()}); "
                                     f"дополнительный CommunityV3Adapter проверил чистый приход в {len(wallet['cases'])} обменах "
                                     f"после CREATE ({wallet['deployment_gas']} газа). Адрес после развёртывания — `dex.v3_wallet_module`. "
                                     f"[Суммы и квитанции](../{(data_dir/'wagmi_thick_modules.json.gz').as_posix()})")
                else:
                    task += " · установлена фабрика этого протокола и сети; торговый профиль требует проверки"
                if kind == "fee_factory":
                    task += " · адрес взят из реестра комиссий источника"
            elif kind == "subgraph":
                task += " · конкретный источник использует подграф; адрес его физической фабрики здесь отсутствует"
            elif kind == "absent":
                task = "В сетевой записи этого адаптера нет данной сети. Отдельные развёртывания проверяются по своему реестру."
        if not base and str(row["id"]) == "2782":
            index = json.loads(gzip.decompress((data_dir/"aktionariat_ethereum_directory.json.gz").read_bytes()))
            fills = json.loads(gzip.decompress((data_dir/"aktionariat_ethereum_protected.json.gz").read_bytes()))
            settled = [r for r in fills["controls"] if "received" in r]
            task = (f"Профиль `aktionariat-ethereum`. Ethereum {index['height']}, реестр 43 пар и два дополнительных брокера эмитентов. "
                    f"Проверки собственной оплаты дали {sum(r['buying'] for r in settled)} покупок и "
                    f"{sum(not r['buying'] for r in settled)} продаж после пополнения рынка прямым платежом. "
                    "При продаже одной акции старый Market выплатил 143,194545 из показанных 144,495 XCHF. Удержано 0,9%. "
                    "Дробная часть акции не покупается. Для нового кошелька разверните AktionariatWallet из карточки DEX, "
                    "адрес сохраняется в dex.router. Валюты рынков XCHF и ZCHF, для расчёта в USDT требуется исполнимый обмен валюты. "
                    "Для обычного графа включите dex.intermediate_enabled и dex.intermediate_wrapped_bridge, задайте dex.router_mode=graph и dex.route_max_hops ≥ 3. "
                    "В dex.intermediate_tokens разрешите WETH, XCHF (0xb4272071ecadd69d933adcd19ca99fe80664fc08) и ZCHF (0xb58e61c3098d85632df34eecfb899a1ed80921cb). "
                    "Для промежуточных токенов без биржевого тикера включите dex.intermediate_dex_priced. Укажите собственный V5 и свой предел check.probe_usdt. "
                    "Курс CHF для пробы и глубины берётся из фактических пулов через WETH. На 1/10 акциях XCHF-рынка внесено 76,455466/892,908956 USDT, "
                    "возвращено 75,252283/880,346284 USDT. На ZCHF-рынке внесено 7,517805/75,209974 USDT, возвращено 7,465294/74,684750 USDT. "
                    "Газ покупки и продажи одной акции составил соответственно 461013+443555 и 457255+361914 единиц. Остаток ZCHF учитывается отдельно от USDT. "
                    "[Восемь платежей по трём пулам и проверки min+1](https://github.com/TGG3Consulting/Arbitraj/actions/runs/46002824880). "
                    "За 7 201 блок в 5 транзакциях эмитенты передали акции 96 раз. В этих транзакциях движения CHF отсутствуют. "
                    "Вторичный рынок с заявками и подписями учитывается отдельно от публичных Brokerbot. "
                    "[Реестр](../tests/data/first_run_dex_2026-11-26/aktionariat_ethereum_directory.json.gz) · "
                    "[Оплаченные входы и отказы](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45998983838).")
        if not base and str(row["id"]) == "6815":
            wallets = data_dir / "shibaswap_v2_ethereum_payments.json.gz"
            book = json.loads(gzip.decompress(wallets.read_bytes()))
            profile = configured["shibaswap-v2-ethereum"]["settings"]
            assert profile["dex.factory"] == book["factory"] and profile["dex.module_runtime_hashes"] == [book["module_hash"]]
            assert set(profile["dex.v3_fee_tiers"]) == set(book["tiers"])
            task = (f"Профиль `shibaswap-v2-ethereum` · полный реестр {len(book['markets'])} пулов по корню хранилища. "
                    f"На {len(book['sampled_markets'])} пулах получено {len(book['cases'])} выплат тремя путями. "
                    f"Штатный роутер принимает активы из dex.router_exact_tokens. Для остальных есть проверка чистого прихода "
                    f"через модуль кошелька или шаг 17 V5. Кнопка размещения сохраняет модуль в dex.router, "
                    f"штатный роутер остаётся в dex.v3_native_router. Для одиночной ERC20-сделки восстановление сверяет канонический "
                    f"вызов и выплату. При удержании нужен корневой результат трассировки, для V5 также balanceOf владельца "
                    f"до и после транзакции и исходные остатки обоих токенов исполнителя. Недоступность этих данных оставляет "
                    f"операцию на сверке. [Суммы, расходы и квитанции](../{wallets.as_posix()}) · "
                    f"[Канонические балансы по индексу транзакции](../{(data_dir / 'shibaswap_historical_returns.json').as_posix()})")
        if str(row["id"]) == "1447":
            assert hashflow["factory"] in {a.lower() for a in candidates}
            candidates = [hashflow["factory"], hashflow["router"]]
            task = (f"Hashflow RFQ · фабрика источника совпадает с factory() официального роутера. "
                    f"В часовом срезе {len(hashflow['events'])} локальных Trade. "
                    "В суточном агрегате также учитываются исходящие межсетевые операции. См. раздел Hashflow четырёх сетей.")
        if not base and str(row["id"]) == "6483":
            task = "Angstrom L1 · 2 физических ключа Uniswap V4, 12 архивных оплат на открытом блоке. Модуль кошелька · 12 подтверждений полного расхода и чистого выхода; требуются размещение и аттестация целевой высоты"
        if str(row["id"]) == "4752":
            import maverick_two_inventory
            ref, candidates, task = maverick_two_inventory.inventory(network)
        if str(row["id"]) == "5221":
            import base_raindex_markets
            candidates, task = base_raindex_markets.inventory(network)
        if match := king_shark.get(str(row["id"])):
            ref = match["source"]
            candidates = [scope["factory"] for scope in match["scopes"]]
            if match["id"] == "7304":
                task = "Те же три реестра SectorOne · " + ", ".join(
                    f"{scope['generation']}: {scope['registered']} пар, {scope['paid_legs']} подтверждённых выплат на выборке"
                    for scope in match["scopes"]) + ". Отдельного пула от имени KING не найдено"
            else:
                scope = match["scopes"][0]
                task = (f"SharkSwap Router02 и V5 · {scope['registered']} CREATE2-пары; на двух рынках {scope['paid_legs']} "
                        "платежей исходного и обратного направления, точный вход, min+1 и истечение срока. "
                        "Штатный путь сравнивает ERC20, включая WETH. Форма выплаты ETH использует весь WETH роутера и в эти измерения не входит. DefiLlama учитывает только объём, историческое распределение комиссии не установлено")
        if base and str(row["id"]) == "7077":
            curve = base_launch_inventory.evidence("stabull_base_directory.json.gz")
            assert curve["factory"]["address"] in [addr.lower() for addr in candidates]
            task = "Stabull · 12 зарегистрированных Curve, кошельковые выплаты по 6; точный вход защищают модуль или V5"
        if base and str(row["id"]) == "7945":
            ryze = read(Path("tests/data/first_run_dex_2026-11-26/base_recovered_markets.json"))["ryze"]
            candidates = [ryze["factory"]["address"]]
            task = "RYZE-RECON · адаптер объёма читает Swap двух пулов этой фабрики; 9 подписанных сделок сопоставлены с движениями кошельков"
        if str(row["id"]) in {"2644","7882"}:
            kind,role=("maverick","Factory") if str(row["id"])=="2644" else ("topaz","CLFactory")
            snapshot=base_launch_inventory.evidence(f"{kind}_v1_{network}_directory.json.gz")
            candidates=[snapshot["roles"][role]["address"]]
            task=f"DEX-46 · {len(snapshot['markets'])} зарегистрированных рынков, см. кошельковые результаты выше"
        if not base and str(row["id"]) in {"7037","7038"} and "degenswap-ethereum" in configured:
            snapshot=cypher if str(row["id"])=="7037" else cl
            candidates=[snapshot["factory"]["address"]]
            task=f"Cypher · {len(snapshot['markets'])} действительных регистраций, испытания и границы выше"
        if base and str(row["id"]) in {"3361", "3888"}:
            is_v3 = str(row["id"]) == "3888"
            profile = configured["alien-base-v3" if is_v3 else "alien-base"]["settings"]
            candidates = [profile["dex.factory"]]
            if is_v3:
                task = ("Alien Base V3 · фабрика из источника объёма совпадает с профилем. "
                        "[Выплата и возврат неизрасходованного WETH в пуле AERO]"
                        "(../tests/data/first_run_dex_2026-11-26/alien_base_750.json.gz)")
            else:
                task = "Alien Base V2 · секция alienbase для Base указывает фабрику профиля и 0,16% на обмен"
        if not base and str(row["id"]) == "7982":
            directory = base_launch_inventory.evidence("bopamm_ethereum.json.gz")
            frames = base_launch_inventory.evidence("bopamm_ethereum_wallets.json.gz")
            registry = directory["directory"]
            market = registry["logs"][0]["address"]
            candidates = [market, registry["roles"][market]["pricing"]]
            trials = [c for frame in frames for c in frame["cases"]]
            paid = [c["after"] for c in trials if c["after"]["status"] == "0x1"]
            before = sum(c["before"]["status"] == "0x1" for c in trials)
            split = sum(len({f["maker"] for f in c["fills"]}) > 1 for c in paid)
            public = base_launch_inventory.evidence("bopamm_ethereum_published.json.gz")
            legs = [c for builder in public for f in builder["frames"] for c in f["cases"]]
            named = [c for c in legs if c["after"]["status"] == "0x1"]
            alternate = sum(c["public_router"]["venue"] != market for c in legs)
            gas_gap = [c["after"]["gas"] - c["public_router"]["gas"] for c in named]
            assert all(c["after"]["net"] == c["public_router"]["net"] for c in named)

            task = (f"bopAMM · {len(registry['roles'][market]['pairs'])} пар текущего рынка. "
                    f"На блоках {registry['start']}–{registry['height']} записано {len(registry['logs'])} обменов. "
                    f"Собственный кошелёк получил WETH или USDC в {len(paid)}/{len(trials)} симуляциях "
                    f"после исходного подписанного обновления и в {before}/{len(trials)} до него. "
                    f"В {split} случаях за одну сделку заплатили два маркетмейкера. "
                    f"Расход защищённого вызова {min(x['gas'] for x in paid)}–{max(x['gas'] for x in paid)} gas. "
                    f"На кадрах Titan и BuilderNet, опубликованных до двух целевых блоков, кошелёк получил выплату в "
                    f"{len(named)} из {len(legs)} проверенных обменов через bopAMM, все WETH↔USDC. "
                    f"LambdaClass дал тот же результат на {min(gas_gap)}–{max(gas_gap)} gas дешевле. "
                    f"Во всех {alternate} проверках с физическим USDT платёж прошёл у LambdaClass через Uniswap V3. "
                    "В маппинге кошелька кнопка «Проверить цену bopAMM» сверяет выбранного строителя, слот и "
                    "актуальный канонический блок. Для проверки чистого минимума нужен собственный BopAmmWallet. "
                    "[Результаты на публичных кадрах](../tests/data/first_run_dex_2026-11-26/bopamm_ethereum_published.json.gz) · "
                    "[Реестр и контроль Uniswap](../tests/data/first_run_dex_2026-11-26/bopamm_ethereum.json.gz) · "
                    "[Оплаты и границы слота](../tests/data/first_run_dex_2026-11-26/bopamm_ethereum_wallets.json.gz).")
        if not base and str(row["id"]) == "405":
            fixture = Path("tests/data/first_run_dex_2026-11-26/tokenlon_ethereum.json")
            tokenlon = read(fixture)
            measure_file = fixture.with_name("tokenlon_ethereum_wallets.json.gz")
            tokenlon_wallet = json.loads(gzip.decompress(measure_file.read_bytes()))
            assert tokenlon_wallet["selector"] == tokenlon["selector"]
            paid = [x for x in tokenlon_wallet["controls"] if x["replay"] == "paid"]
            window = tokenlon["window"]
            sources = window["fills_by_source"]
            external = sum(sources[name] for name in ("uniswap_v2", "uniswap_v3", "curve", "sushiswap"))
            candidates = [tokenlon["roles"][name]["address"] for name in ("pmm", "wrapper", "rfq6")]
            task = (f"Блоки {window['first']}–{window['last']}. Число оплат из инвентаря маркет-мейкеров PMM — "
                    f"{sources['pmm_zeroex_v2']}. Сделок через внешние AMM — {external} (Uniswap, "
                    "Curve и SushiSwap). Публичная цена PMM доступна через SDK. При запросе окончательной заявки "
                    "три пустых кошелька получили USER_BALANCE_INSUFFICIENT. Для допуска требуется действующая "
                    "подпись маркет-мейкера и симуляция выплаты своему кошельку. "
                    f"При воспроизведении {len(paid)} из {len(tokenlon_wallet['controls'])} исторических сделок "
                    f"показали точный дебет и чистое поступление ({min(x['gas'] for x in paid)}–{max(x['gas'] for x in paid)} gas). "
                    f"[Ответы сервиса](../{fixture.as_posix()}) · [Оплата и повтор](../{measure_file.as_posix()}) · "
                    "[Официальный порядок получения котировки](https://docs.token.im/tokenlon-jssdk/en/#getquote)")
        if not base and str(row["id"])=="162":
            fixture = Path("tests/data/first_run_dex_2026-11-26/bancor_v21_ethereum.json.gz")
            original = json.loads(gzip.decompress(fixture.read_bytes()))
            candidates = [original["roles"][key]["address"] for key in ("BancorConverterRegistry","BancorNetwork")]
            paid = sum("received" in case for case in original["controls"])
            task = (f"Bancor V2.1 · {len(original['markets'])} якорей с владельцами-конвертерами в исходном реестре. "
                    f"{paid} выплат при прямых и составных обменах во внешнем архиве "
                    f"[на блоке {original['block']}](../{fixture.as_posix()}).")
        if not base and str(row["id"])=="6771":
            fixture=Path("tests/data/first_run_dex_2026-11-26/yield_basis_ethereum.json.gz")
            original=json.loads(gzip.decompress(fixture.read_bytes()))
            cases=[r for venue in original["venues"].values() for r in venue["cases"]]
            virtual=[r for venue in original["venues"].values() if venue["kind"]=="virtual_pool" for r in venue["cases"] if "received" in r]
            candidates=[original["factory"]]
            task=(f"YieldBasis · {len(original['markets'])} рынков, {len(original['venues'])} контрактов обмена. "
                  f"На блоке {original['block']} получено {sum(r['stage']=='paid' for r in cases)} выплат исходных пулов. "
                  f"Фактическое поступление отличается от get_dy в {sum(r['preview']!=r['received'] for r in virtual)} из {len(virtual)} выплат VirtualPool. "
                  f"[Объёмы и кошелёк](../{fixture.as_posix()})")
        if not base and str(row["id"])=="116":
            book_file=Path("tests/data/first_run_dex_2026-11-26/balancer_v1_ethereum.json.gz")
            with gzip.open(book_file,"rt",encoding="utf8") as stream:
                original=json.load(stream)
            candidates=[original["factory"]]
            task=f"Balancer V1 · {len(original['registered'])} оригинальных BPool. Обе формы обмена и ограничения по округлению сверены с исходным пулом"
        if not base and str(row["id"])=="7520":
            task="Карточные платежи; адаптер crypto-card.ts измеряет использование карты. Отдельные спотовые реестры Cypher указаны под 7037 и 7038"
        if not base and str(row["id"]) == "9435":
            directory = read(Path("tests/data/first_run_dex_2026-11-26/goliath_ethereum.json"))
            candidates = [directory["venue"]]
            task = "ETH-GOLIATH · две связки с WETH, активность зависит от блока"
        if not base and str(row["id"]) == "9157":
            directory = read(Path("tests/data/first_run_dex_2026-11-26/community_v3_ethereum_router.json"))
            candidates = [directory["factory"]]
            task = "[Подтверждено командой CommunityV3](https://forum.cow.fi/t/communityv3-liquidity-source-integration-proposal/3647)"
        if base and str(row["id"]) == "3":
            archive = json.loads(gzip.decompress((data_dir / "curve_base_crypto_v1.json.gz").read_bytes()))
            candidates = list(archive["factories"].values())
            task = ("Curve API публикует объём сети, резервный расчёт использует пять фабрик. "
                    "На блоке 62 262 756 реестры дали Stable 15, Crypto v1 34, TwoCrypto 276, TriCrypto 95, Stable-NG 756. "
                    "В Crypto v1 33 клона проверенного шаблона. В четырёх пулах обмены от WETH и обратно подтвердили "
                    "16 выплат через V5 и 16 через пул. У WETH/cbETH в резерв входят native ETH, у двух проверенных "
                    "размеров пятого рынка первая операция откатилась. Для торговли используется V5 шаг 9 с общим сроком "
                    "и чистым минимумом. Сетевой оборот отдельно по фабрикам не распределялся.")
        if str(row["id"]) == "8322":
            fx=json.loads(gzip.decompress((data_dir/"textile_fx.json.gz").read_bytes()))
            chain=fx["chains"][network]
            candidates=[chain["roles"]["reactor"]["address"]]
            task=(f"Textile FX · в {chain['window'][1]-chain['window'][0]+1} блоках зарегистрировано {chain['fills']} Fill. "
                  "Комиссия 2 б.п. включена во вход и округляется по каждой заявке maker. "
                  "Preview использует опубликованные уровни без блокировки средств. Индивидуальный запрос владельца резервирует объём. "
                  "В отдельном контроле на действующих Reactor и Permit2 выполнены восемь выплат на сеть: два направления, один или два maker, "
                  "прямой кошелёк и защищённый TextileWallet. Контрольные подписи истекли. "
                  "Торговый выбор получает только подтверждённый объём и срок явно разрешённой владельцем заявки. "
                  "Для защищённого модуля нужен согласованный агрегаторский доступ. Из иных форм расчёта Aave и отложенный GMX требуют своих проверок. "
                  "[API Textile](https://api.textilecredit.com/v2/openapi.json) · "
                  f"[Проверки платежа](../tests/data/first_run_dex_2026-11-26/textile_{network}_wallets.json.gz).")
        if base and str(row["id"]) == "6625":
            snapshot = json.loads(gzip.decompress((data_dir / "sportfun_base_players.json.gz").read_bytes()))
            candidates = [p["address"] for p in snapshot["pairs"]]
            players = sum(p["funded"] for p in snapshot["pairs"])
            task = (f"Sport.fun Pro · {players} ID игроков ERC-1155 с резервом USDC на двух площадках. "
                    "В последних 1000 блоках записаны четыре события покупки, продажи и обмена. "
                    "Две записи одного обмена относятся к одной транзакции. Игрок получает доли по ID через назначенный пул "
                    "с подписью котировки. Условия ограничивают передачу долей и автоматизированный доступ. "
                    f"[Правила Sport.fun, §§ 2 и 11]({snapshot['rules']})")
        if base and str(row["id"]) == "382":
            directory = json.loads(gzip.decompress((data_dir / "pendle_base_registry.json.gz").read_bytes()))
            candidates = [r["address"] for name, r in directory["roles"].items() if name.startswith("marketFactory")]
            task = ("Две официальные фабрики дали 90 регистраций PT/SY, из них 22 профиля серии на блоке 62 262 756. "
                    "На двух рынках 1 и 10 USDC оплатили PT. Обратный выход доступен в собственный токен доходности или SY. "
                    "Модуль и V5 провели по 16 обменов. На рынке с доступом к USDT четыре проверенных размера отклонились. "
                    "Для исполнения владелец выбирает размещение модуля, вносит адрес в dex.router и выбирает "
                    "dex.kind=pendle_market_v5. [Реестр и версии](https://docs.pendle.finance/pendle-v2-dev/Deployments)")
        if not base and str(row["id"]) == "382":
            directory = json.loads(gzip.decompress(Path("tests/data/first_run_dex_2026-11-26/pendle_ethereum_registry.json.gz").read_bytes()))
            candidates = [r["address"] for name, r in directory["roles"].items() if name.startswith("marketFactory")]
            task = "Пять фабрик PT/SY и 690 регистраций, см. ETH-PENDLE"
        if not base and str(row["id"])=="2890":
            candidates=[carbon["roles"][0]]
            task="Carbon DeFi · официальный Analytics API агрегирует USD по сети. Приведённый контроллер v6 отдельно подтверждён событиями и кодом"
        if not base and str(row["id"]) == "292":
            candidates = ["0xd34971bab6e5e356fd250715f5de0492bb070452"]
            task = "SWAPR-01 · 44 пары, две ставки. Оба маршрута прошли по двум размерам и направлениям"
        if not base and str(row["id"]) == "9436":
            stelaxis = read(Path("tests/data/first_run_dex_2026-11-26/stelaxis_ethereum.json"))
            candidates = [stelaxis["venue"]]
            task = "STEL-01 · 5 пар, 100 событий в 250 блоках. Собственный исполнимый маршрут не подтверждён"
        if not base and str(row["id"])=="7382":
            candidates=[sn["factory"]]
            task=f"Supernova CL · {len(sn['markets'])} рынков в четырёх реестрах customDeployer по CREATE2"
        if not base and str(row["id"])=="4062":
            candidates=[dx["proof"]["factory"]["address"]]
            task="DeltaSwap Ethereum · сверены 17 рынков фабрики и выплаты официального Router02"
        if base and str(row["id"]) == "8934" and baibai.exists():
            candidates = [bai["deployments"]["entry"]["address"]]
            task = "DEX-25 · Entry подтвердил выплаты и комиссии WETH/USDC и NVDAc/USDC. В обоих рынках проверено по две стороны и два объёма"
        if base and str(row["id"]) == "7601" and lunar.exists():
            task = "DEX-24 · один действующий native ETH/USDC, фактические implementation и выплаты по обеим сторонам; три размещения приостановлены"
        if base and str(row["id"]) == "8849":
            homelander = read(Path("tests/data/first_run_dex_2026-11-26/homelander_base.json"))
            markets = homelander["markets"]
            count = sum(m["matches_active"] for m in markets)
            task = (f"DEX-26 · у {count}/{len(markets)} пулов Hydrex и QuickSwap активный hook совпал с реестром Homelander. "
                    "Адаптер объёма маркирован doublecounted. Тот же обмен учитывается в исходном пуле")
        if str(row["id"]) == "9154":
            nami = read(Path(f"tests/data/first_run_dex_2026-11-26/nami_{network}.json"))
            candidates = [nami["config"]["factory"]]
            count = sum(len(m["cases"]) for m in nami["markets"])
            task = (f"DEX-27 · фабрика {nami['config']['factory']}, квотер {nami['config']['quoter']}; "
                    f"{count} архивных полных обменов V5. "
                    + ("8/8 обменов изменили собственный пул." if base else
                       "4/4 обмена дополнительно изменили пул Uniswap V3 0x88e6a0c2ddd26feeb64f039a2c41296fcb3f5640.")
                    + " Исполнитель сравнивает одиночный Nami с обычным сплитом на тот же объём и направляет сюда только отдельный обмен")
        if str(row["id"])=="9267":
            record=base_launch_inventory.evidence("fynd_"+network+"_markets.json.gz")
            candidates=[record["core"],*record["deployments"]]
            task=f"DEX-45 · {len(record['markets'])} физических ключей Fynd. Каждая сделка требует действующей подписи контроллера и разрешённого locker; см. условия и подключение Fynd AMM."
        if base and str(row["id"])=="9327":
            candidates=[base_launch_inventory.evidence("vecta_base_directory.json")["roles"]["factory"]["address"]]
            task="DEX-45 · три рынка VectaSwap; V5 и штатный Router выплатили проверенные суммы на EURC/USDC. Профиль vectaswap-base."
        if base and str(row["id"])=="8513":
            task="DEX-44 · UmiaHub, четыре текущих и один прежний спотовый vault. Общий объём включает решения UmiaMarketCore, по которым нужны исход и погашение. См. фактические выплаты в разделе Umia."
        if base and str(row["id"])=="4725":
            task="DEX-43 · адрес в адаптере исключён из подсчёта объёма. Три физических реестра и 88 рынков приведены в разделе Spectra; торговые монеты — IBT/PT."
        if base and str(row["id"]) == "7260" and elfomo.exists():
            task = "DEX-23 · 12 пар в реестре ElfomoFi. Подтверждены прямые выплаты WETH/USDC и cbBTC/USDC, нулевые котировки USDT/USDC"
        if base and str(row["id"]) == "9172" and aero_basic.exists():
            candidates = [f["factory"] for f in ab["factories"]]
            task = "DEX-22 · обе фабрики из POOL_FACTORIES в адаптере объёма сверены с официальным размещением и реестрами Base"
        if base and str(row["id"]) == "7984" and kipseli.exists():
            task = "DEX-21 · QuoteLens и текущая реализация подтверждены. Исторический адаптер объёма перечисляет три другие реализации. Исполнение ожидает доступа оператора"
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
        if scope := top.get(str(row["id"])):
            ref = f"https://github.com/DefiLlama/dimension-adapters/blob/{top_data['revision']}/{scope['path']}"
            # Сторонние SDK и служебные адреса на соседних сетях не являются торговыми доменами данной строки.
            domains = [d["address"] for d in scope["domains"]]
            source_addresses = scope["other_source_addresses"]
            if domains or source_addresses:
                candidates = list(dict.fromkeys(domains + source_addresses))
            task = f"LL20 · {scope['description']}"
        if str(row["id"]) == "622":
            fixture = data_dir / f"clipper_{network}_directory.json.gz"
            with gzip.open(fixture, "rt", encoding="utf8") as stream:
                blade = json.load(stream)
            pools = [p["pool"] for p in blade["pools"]]
            registry = blade["registry"]
            assert registry["complete"] and registry["storage_root"] == registry["reconstructed"]
            candidates = list(dict.fromkeys([*candidates, *pools, registry["address"]]))
            task = (f"Clipper / Sushi Blade · на блоке {blade['height']} подтверждены все адреса реестра "
                    f"по корню хранилища. Подписываемых пулов в исходном перечне сети и реестре: {len(pools)}. "
                    "0x663f…c7f — пул на Base и разрешающий роутер на Ethereum. ")
            if not base:
                legacy = blade["unsigned_legacy"]
                assert not legacy["enabled"] and not legacy["payer_blocked"] and not legacy["payer_allowed"]
                task += ("Старый on-chain пул показывал 0,998237 и 9,980753 USDT за 1 и 10 USDC. "
                         "Обе профинансированные покупки отклонил контракт: swapsAllowed=false; кошелёк сохранил USDC. "
                         "В исходном Cove не найдено ни одного события депозита или обмена от создания по этот блок. ")
            task += (f"[Адреса, роли, остатки и квитанции](../{fixture.as_posix()}) · "
                     "[Официальный список Sushi](https://github.com/sushi-labs/clipper-rfq-subgraph/tree/453c83478f2553270f4fcf52797695fc52553240/scripts/deploy-codegen/networks).")
        classic_ids = {"119": "sushiswap-v2-base" if base else "sushiswap",
                       "194": "pancakeswap-v2-base" if base else "pancakeswap_ethereum"}
        if classic_id := classic_ids.get(str(row["id"])):
            fixture = data_dir / f"classic_spot_wallets_{network}.json.gz"
            with gzip.open(fixture, "rt", encoding="utf8") as stream:
                classic = json.load(stream)[classic_id]
            assert classic["factory"] in {a.lower() for a in candidates}
            paths = [c["wallet_paths"] for c in classic["cases"]]
            deltas = [x-y for p in paths for x,y in zip(p["executor"]["swap_gas"],p["router"]["swap_gas"])]
            task = (f"Профиль `{classic_id}` · WETH/USDC `{classic['pool']}`. Два размера покупки и продажи. "
                    f"Каждый платёж оригинального роутера и V5 совпал с расчётом при {classic['pips']} ppm; "
                    f"при той же выплате оригинал сэкономил {min(deltas)}–{max(deltas)} gas за своп. "
                    f"Обратная проверка пары и minOut +1 отклонили завышенный выход в обе стороны. "
                    f"[Квитанции и сроки](../{fixture.as_posix()}).")
        if not base and str(row["id"]) == "812":
            fixture = data_dir / "llama_eth_v2_wallets.json.gz"
            with gzip.open(fixture, "rt", encoding="utf8") as stream:
                empire = json.load(stream)["empire"]
            assert empire["factory"] in {a.lower() for a in candidates}
            task = (f"EmpireDEX · фабрика подтвердила {len(empire['pools'])} пар. "
                    f"[Реестр](../{fixture.as_posix()}) · [Платежи и контроль защищённого маршрута](../tests/data/first_run_dex_2026-11-26/empire_ethereum_protected.json.gz).")
        out.append(f"| {row['id']} | {cell(row['name'])} | {volumes.get(row['id'], 'нет данных')} | [{cell(row['module'])}]({ref}) "
                   f"| {addresses(candidates)} | {task} |")
    scope_note = (
        "Из Base двадцатки обнаружено одно дополнительное размещение уже известной площадки. "
        "У ранней фабрики Aerodrome Slipstream 22 пула, у двух положительна активная ликвидность. "
        "За сутки с 61 830 752 по 62 262 756 найден один Swap. Допуск ранней фабрики опирается на [платежи двух кошельковых маршрутов](../tests/data/first_run_dex_2026-11-26/early_slipstream_wallets.json.gz). У Hanji найдено три книги адаптера объёма, одна из них рассчиталась с кассой из штатного API. [Регистрация и фактическая выплата](../tests/data/first_run_dex_2026-11-26/llama_top_hanji.json.gz). Karta учитывает карточные расчёты, "
        "а Homelander повторно учитывает пулы сторонних AMM."
        if base else
        "В Ethereum двадцатке подтверждена отдельная касса Kipseli. Из 1452 событий за сутки "
        "на одном обмене прослежены 3342,773397 USDC в кассу и 3346,310065 USDT получателю. "
        "Через публичный Ethereum PropAMMRouter один EOA продал WETH за USDC в трёх сделках. В диапазоне 26444589–26494988 "
        "роутер указал Kipseli 732 раза по четырём парам USDC с WETH, WBTC, cbBTC и USDT. Фактический путь входа: "
        "кошелёк → venue → pool → logic → Safe, выход идёт из того же Safe получателю. Обычная заявка разрешает "
        "резервный Uniswap; в квитанции учитывается фактическая площадка. Котировка зависит от цены в блоке. "
        "В архивном прогоне 45954708341 на двух блоках 8 из 8 обычных и 8 из 8 защищённых заявок с разрешённой заменой заплатил Uniswap. Все восемь заявок с обязательным Kipseli откатились. Владелец может отдельно разместить PropVenueAdapter с принятым runtime `0x5fc58eccc0b51ee3dfac7730ce3e28257f990cbdee18db91846c90e4791a9d67`. [Архив платежей](../tests/data/first_run_dex_2026-11-26/kipseli_ethereum_wallets.json.gz). "
        "В Base действуют отдельные ключ API и разрешение на подпись. У 0x публичный GET цены и перечня источников без ключа "
        "вернул 401 27.11.2026, как и Base Kipseli GET глубины. [Три оплаченных поручения, роли и ответы API](../tests/data/first_run_dex_2026-11-26/kipseli_ethereum.json.gz). "
        "У Ondo подтверждены события выпуска и погашения, "
        "у 0x — отдельная ликвидность маркетмейкера RFQ. В смешанной транзакции он получил 0,057127313126944295 WETH и выплатил токен из своей кассы; две предшествующие ноги исполнил Uniswap V4. [Квитанция и регистрация 0x Settler](../tests/data/first_run_dex_2026-11-26/llama_top_0x_rfq.json.gz)."
    )
    out.extend(["", scope_note + " [Сопоставление двадцатки](../tests/data/first_run_dex_2026-11-26/llama_top_bindings.json) "
                "· [Канонические рынки и движения средств](../tests/data/first_run_dex_2026-11-26/llama_top_contracts.json.gz)."])
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


def alien_alternative_fees():
    """Сопоставляет платы по одинаковой исходной заявке через независимые зарегистрированные пулы."""
    directory = Path("tests/data/first_run_dex_2026-11-26")
    with gzip.open(directory / "alien_fee_comparison_base.json.gz", "rt") as stream:
        replay = json.load(stream)
    with gzip.open(directory / "alien_fee_discovery_base.json.gz", "rt") as stream:
        probe = json.load(stream)["mixed_fee_probe"]
    found = probe["found"]
    totals = sum(map(len, found.values()))
    old = [len(set(pools) - {"750"}) for pools in found.values()]
    out = ["### Alien Base V3 · WETH/USDC", "",
           f"На блоке {replay['block']} проверены 45 сочетаний десяти токенов. Фабрика вернула {totals} физических пулов "
           f"для {len(found)} пар. Прежние шесть ставок охватывали {sum(old)} пулов для {sum(n > 0 for n in old)} пар. "
           "Ставка 750 со spacing 15 добавила WETH/AERO и WETH/USDT и ещё четыре альтернативы для уже обнаруживаемых пар. "
           "Событие PoolCreated и индексатор дают отдельные пути обнаружения. Адреса WETH/AERO и WETH/USDC найдены "
           "прямым запросом GeckoTerminal 27 ноября. Попадание этих пулов в выдачу по токену у работающего бота 25 ноября неизвестно. "
           "[Канонические вызовы и ответы индексатора](../tests/data/first_run_dex_2026-11-26/alien_fee_discovery_base.json.gz).", "",
           "V5 обменял одинаковые суммы между WETH и нативным контрактом USDC Base. Входы USDC заранее получены на отдельной паре Uniswap V3. "
           "В каждой ячейке ниже приведены чистое зачисление кошельку в минимальных единицах выходного токена и газ вызова swap. "
           "Каждая оплаченная нога проверена с точным минимумом, минимумом на единицу выше и истекшим сроком.", "",
           "| Вход | Количество в минимальных единицах | Ставка 200 | Ставка 400 | Ставка 750 |", "|---|---:|---:|---:|---:|"]
    fills = {}
    for market in replay["markets"]:
        for attempt in market.get("attempts", []):
            leg = attempt.get("buy" if attempt["source"] == "WETH" else "sell")
            if leg:
                exact = leg["exact"]
                fills.setdefault((attempt["source"], int(attempt["size"])), {})[market["tier"]] = (
                    f"{exact['received']} / {int(exact['gas'], 16)}")
    for (asset, size), tiers in sorted(fills.items()):
        out.append(f"| {asset} | {size} | {tiers[200]} | {tiers[400]} | {tiers[750]} |")
    return out + ["", "Пулы 3000 и 10000 отклонили все четыре исходных размера. Ставка 750 дала наибольший выход в четырёх сравнениях. "
                  "Для покупки WETH за 1 USDC она потребовала на 2052 единицы газа больше ставки 400. Расходы на разрешение, газ в валюте сети, "
                  "L1 и перевод результата в USDT учитываются отдельно при выборе исполнимого маршрута. "
                  "[Квитанции, расходы и границы](../tests/data/first_run_dex_2026-11-26/alien_fee_comparison_base.json.gz) · "
                  "[Архив GitHub Actions](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45979186408).", ""]


def stabull_markets():
    root = Path("tests/data/first_run_dex_2026-11-26")
    def capture(name):
        with gzip.open(root / f"stabull_base_{name}.json.gz", "rt", encoding="utf8") as stream:
            return json.load(stream)
    directory = capture("directory")
    protected = capture("protected_wallets")
    native = capture("native_wallets")
    oracle = read(root / "stabull_base_oracles.json")
    earlier, current = oracle["samples"]
    ages = [current["timestamp"] - r["round"][3] for r in current["roles"]]
    changed = sum(a["rate"] != b["rate"] for a, b in zip(earlier["roles"], current["roles"], strict=True))
    out = ["### Stabull на Base", "",
           f"Фабрика `{directory['factory']['address']}` создала 12 Curve к блоку {directory['height']}. "
           f"За 24 часа зарегистрировано {directory['events']} Trade. Официальный Router `{directory['factory']['router']}` "
           "принимает точный вход. Curve принимает также целевую выплату с максимальным входом. "
           "Внешний оракул может обновиться между блоками без события Curve. Каждый блок заново читаются флаги, обе валюты, assimilator и их курсы. "
           f"На высотах {earlier['height']} и {current['height']} для всех {len(current['roles'])} assimilator проверены token, oracle, getRate и latestRoundData. "
           f"Значения изменились у {changed}; на последней высоте возраст ответов от {min(ages)} до {max(ages)} секунд. "
           "Доказательства опубликованного срока по каждому оракулу в настройках отсутствуют.",
           "", "Первая настройка `dex.kind=stabull` загружает котировки. Для денежного пути владелец размещает "
           "StabullAdapter. В настройках семейства откройте «Разместить StabullAdapter на Base», затем «Открыть параметры размещения». "
           "Скачайте неподписанный CREATE или подключите выбранный кошелёк Base, оцените газ и подтвердите создание. "
           "При потере ответа укажите хеш исходной операции либо замены с тем же плательщиком и nonce. "
           "После финализации форма сверяет сеть, код фабрики и runtime созданного контракта. Сохраните подтверждённый адрес "
           "в `dex.router`, затем выберите `dex.kind=stabull_module` и сохраните настройки. "
           f"Архивный runtime `{protected['module']['runtime']}`, расход размещения {protected['module']['deployment_gas']} gas. "
           "Прямой модуль проверяет фактический расход и выплату при точном входе и целевом выходе. Котировка viewTargetSwap задаёт первую пробу; "
           "при расхождении кошельковая симуляция определяет точный бюджет и повторно проверяет его. Во время восстановления сверяются первоначальная calldata, "
           "Curve Trade и движение денег владельца. Официальный Router задаётся через `dex.quoter` и `dex.quoter_runtime_hash`. "
           "Его ABI допускает денежный обмен с точным входом. Curve допускает оба способа. Эти прямые ERC20-формы требуют двух "
           "проверенных реализаций в `dex.router_exact_tokens`; все 12 рынков содержат обновляемый Base USDC, автоматический список пуст. "
           "Шаг V5 проверяет срок внешнего поручения; "
           "внутри одной транзакции адаптер согласует его с более строгой проверкой Curve. Лимит капитала, ступени в USD, срок и разрешение торговли задаёт владелец.",
           "", "Архив моделирует 0,001 и 1 USDC и обратную продажу полученной валюты. Счётчик учитывает отдельные подтверждённые выплаты, включая повтор разными входами. "
           "Во всех успешных случаях более строгий денежный предел на один атом и истекший срок отвергнуты. "
           "Для 0xc77c42b… официальный viewTargetSwap сообщил 417659002839075987 атомов входа, "
           "фактическая выплата 499849 атомов USDC потребовала 417659003096570581. Модель выбирает кандидата; подписанный минимум и бюджет сохраняются.",
           "", "| Рынок | Curve | Вход/выход Curve | Router | Модуль | V5 |", "|---|---|---:|---:|---:|---:|"]
    for market, direct, guarded in zip(directory["pools"], native["records"], protected["records"], strict=True):
        assert direct["pool"] == guarded["pool"] == market["address"]
        counts = {m: 0 for m in ["pool", "router", "module", "v5"]}
        for case in direct["cases"] + guarded["cases"]:
            counts[case["mode"]] += sum(case.get(k, {}).get("stage") == "paid" for k in ["buy", "sell", "buy_target", "sell_target"])
        out.append(f"| {market['pool_name']} | `{market['address']}` | {counts['pool']} | {counts['router']} | {counts['module']} | {counts['v5']} |")
    return out + ["", "[Размещения Stabull](https://docs.stabull.finance/amm/contracts) · [Архив кошелька и V5](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45964238282) · [Граница V5: равенство времени успешно на 20 выплатах](https://github.com/TGG3Consulting/Arbitraj/actions/runs/45966291856)", ""]


def hashflow_market_observation():
    """Один одинаковый час наблюдения за именованным Trade на четырёх сетях."""
    data = read(Path("tests/data/hashflow_direct_2026-11-26/markets_hour.json"))
    out = ["### Hashflow в четырёх сетях", "",
           "27.11.2026, 05:00–06:00 UTC. Для каждого эмитента исследованы обе связи с официальным роутером через "
           "`authorizedPools(pool)` и `pool.router()`. Сумма событий относится к указанному часу. "
           f"[Реестр Hashflow]({data['official_manifest']}) · [События и цели проверок](../tests/data/hashflow_direct_2026-11-26/markets_hour.json)", "",
           "| Chain ID | Проверенный роутер | Диапазон блоков | События Trade | Направленные пары |",
           "|---:|---|---:|---:|---:|"]
    for network in data["networks"]:
        out.append(f"| {network['chain']} | `{network['router']}` | {network['first']['number']}–{network['last']['number']} "
                   f"| {len(network['events'])} | {len(network['market_events'])} |")
    return out + ["", "Для рабочего модуля в форме владельца выберите Taker с доступом Multi-Wallet. "
                  "Выданное имя укажите в `hashflow.source`, ключ — в секрете `hashflow.taker_key`. "
                  "Taker API передаёт ключ заголовком Authorization. Подпись RFQ создаёт маркетмейкер, "
                  "а транзакцию оплачивает и подписывает выбранный кошелёк. Установление HMAC для Taker API v3 не требуется. "
                  "Для индивидуального режима в форме выберите Taker с привязкой к одному кошельку. Введите "
                  "`api` в `hashflow.source`, выданный ключ в `hashflow.taker_key`, а зарегистрированный публичный адрес "
                  "в `hashflow.api_wallet`. На GET передаётся wallet, в подписанной RFQ его роль выполняет trader. "
                  "Персональные условия, актуальные рынки и ответы для своего кошелька проверяются после получения доступа. "
                  "[Форма](https://forms.gle/EEzfL59vQaTebAXN9) · "
                  "[Два вида доступа](https://docs.hashflow.com/hashflow/taker/getting-started-api-v3)", ""]


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
    from base_empire_markets import empire_markets
    lines.extend(empire_markets())
    from base_angstrom_markets import angstrom_markets
    lines.extend(angstrom_markets())
    from base_epsilon_markets import epsilon_markets
    lines.extend(epsilon_markets())
    from base_spacing_markets import clone_classic_markets, spacing_markets, virtus_cl_markets
    lines.extend(spacing_markets())
    lines.extend(virtus_cl_markets())
    lines.extend(clone_classic_markets())
    from base_raindex_markets import raindex_markets
    lines.extend(raindex_markets())
    lines.extend(alien_alternative_fees())
    lines.extend(stabull_markets())
    lines.extend(hashflow_market_observation())
    doc.write_text(front + start + "\n\n" + "\n".join(lines) + "\n" + end + tail, encoding="utf8", newline="\n")


if __name__ == "__main__":
    main()
