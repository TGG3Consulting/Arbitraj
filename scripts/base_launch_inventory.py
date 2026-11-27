"""Сводит адреса и результаты запусков Bankr и o1 из канонических архивов без сетевых чтений."""
import gzip
import json
from pathlib import Path

DATA=Path("tests/data/first_run_dex_2026-11-26")

def evidence(name):
    with gzip.open(DATA/name,"rt",encoding="utf8") as f:return json.load(f)

def section():
    cfg=evidence("base_launch_hooks.json.gz")
    rows=[r for r in cfg["rows"] if "terms" in r]
    out=["","#### Bankr, Bankr Index и o1 на Base","",
         f"На контрольной высоте {cfg['height']} событие Initialize менеджера `{cfg['manager']}` и регистрации хуков подтвердили {len(rows)} рынков. "
         "При измерении V4Quoter исполняет обмен с хуком. Для исполнимого маршрута проверяются списание и выплата кошелька, лимит и срок. "
         "Из-за внешних обменов компонентов и реинвестирования комиссий каждый такой рынок котируется отдельно. "
         "Размеры ступеней в USDT и допуск групп хуков задаёт владелец.","",
         "| Площадка | Контрольных рынков | Успешных ног Router / V5 | Газ Router / V5, на ногу | Публичные измерения |",
         "|---|---:|---:|---|---|"]
    for id in ("bankr","bankr-index-base","o1-launchpad"):
        f=id.replace("-","_")+"_base_wallets.json.gz"
        d=evidence(f)
        success={m:[r[leg] for item in d["markets"] for r in item["cases"] if r["mode"]==m for leg in ("buy","sell")
                    if r.get(leg) and r[leg]["stage"]=="accepted"] for m in ("router","v5")}
        gas=["–".join([f"{min(x['gas'] for x in a):,}",f"{max(x['gas'] for x in a):,}"]) for a in success.values()]
        out.append(f"| `{id}` | {len(d['markets'])} | {len(success['router'])} / {len(success['v5'])} | {' / '.join(gas)} | [Списания, выплаты и оба отказа](../{DATA/f}) |")
    return out+["","Bankr использует DopplerHookInitializer и два зарегистрированных RehypeDopplerHookInitializer. У 15 пулов состояние Locked, флаги дочерней роли равны 3. Пары имеют шаг 200 и динамическую комиссию, запись прогрева подтверждается повторным чтением обеих ролей. У девяти торговых токенов попытка лишнего разрешения Permit2 отклонилась; кошелёк использовал уже действующее разрешение.",
        "","Каждый из девяти ключей Index входит в poolsOf(index), а фабрика признаёт vault. На снимке ни один из этих индексов не требовал подписанного пропуска. При включении passRequired покупка исчезает из лесенки. Размер корзины и комиссии влияют на исполнимый объём: в 18 случаях каждого маршрута отклонилась сама котировка, ещё четыре ненулевых котировки на 1000 USDC не завершили оплату у измеренного отправителя.",
        "","У o1 в выборке три реализации Standard. Их PoolKey содержит fee 0, но указанный в poolConfig базовый сбор составляет 100 базисных пунктов. Начальный повышенный сбор до 9900 базисных пунктов и продолжительность окна считываются для каждого пула. Нативный ETH проверен через опубликованный Universal Router; у текущего V5 класс 19 требует две валюты ERC‑20. На трёх ETH рынках исходная валюта платежа отличается от WETH, количество и газ учитываются отдельно. Для ANTc, NVDAc и VVV проверены реальные держатели на каноническом блоке.",
        "", "На трёх физических ETH-рынках дополнительный профиль `o1-launchpad-wrapped` оплачивает сделки из собственного WETH. "
        "Граница полного входа, фактическое зачисление и подписанный PoolId проверены на 12 ногах прямого модуля и 12 ногах V5. "
        "Газ одного обмена составил 188 360–226 641 и 244 448–267 573 соответственно. Владелец задаёт `dex.router` после размещения. "
        "На контрольном блоке оценка размещения при 1 524 909 газе и базовой ставке 5 000 000 wei равна 0,000007624545 ETH за L2, "
        "дополнительно оплачиваются данные L1 и приоритет. Профиль проверяет `dex.module_runtime_hashes` по байткоду с фактическими immutable. "
        f"[Хеш, роли, суммы и контрольные отказы модуля](../{DATA/'o1_wrapped_base_wallets.json.gz'}).",
        "",f"[Initialize, формы ABI, роль и хеш каждого контракта](../{DATA/'base_launch_hooks.json.gz'}) · [Держатели ANTc, NVDAc и VVV](../{DATA/'o1_launch_holders.json'}).",""]

def recovered(network):
    doc=json.loads((DATA/"docswap_empty_directories.json").read_bytes())
    d=doc[network]
    out=["",f"DOCSWAP · `{d['router']}` вернул фабрику `{d['factory']}` и WETH своей сети. В реестре allPairsLength() = {int(d['count'],16)} на контрольном хеше. "
         f"[Код и исходные ответы в обеих сетях](../{DATA/'docswap_empty_directories.json'}) · [Реестр проекта]({doc['source']}).",""]
    if network!="base":return out
    r=json.loads((DATA/"base_recovered_markets.json").read_bytes())
    ry,third,bl=r["ryze"],r["thirdfy"],r["baseline"]
    return out+["#### Повторная проверка каталогов Base","",
        f"Адреса WETH/USDC и cbBTC/USDC из официального Ryze API совпали с двумя элементами pools() фабрики `{ry['factory']['address']}`. "
        f"MultiHopRouter `{ry['router']['address']}` проверяет подписанный ордер; штатное исполнение использует nonce EIP‑712 и данные оракула. "
        "[ABI и назначение контрактов](https://documentation.ryze.pro/guide/abis).",
        "",f"Фабрика Thirdfy Algebra Integral `{third['factory']['address']}` подтвердила каждый из шести рынков. "
        "На каноническом чтении все шесть имели ненулевую активную ликвидность и pluginConfig 193. Комиссия на высоте варьировалась от 3000 до 7386 миллионных. ArbExecutor V5 исполнил 24 ноги на шести рынках, в обоих направлениях и на двух объёмах. "
        f"[Архив кошельковых выплат и повышенных минимумов](../{DATA/'thirdfy_base_wallets.json.gz' }).",
        "",f"Baseline использует bToken `{bl['btoken']['address']}` в качестве идентификатора страницы. Реальный ключ BSR/cbBTC `{bl['pool_id']}` "
        f"создан в транзакции `{bl['index']['createdAtTxnId']}`, хук `{bl['hook']['address']}`. "
        "На 100 и 1000 raw cbBTC владелец получил 26019463245970438 и 260194412075800528 raw BSR. "
        "При продаже этих объёмов вернулись 98 и 980 raw cbBTC. Роутер и V5 дали одинаковый чистый выход во всех восьми ногах. "
        f"Реестр Relay вернул семь компонентов, 113 распределённых ABI-селекторов и разрешённый пул. При расчёте вновь сверяются весь реестр, хеши, маршруты, обратимое соответствие ключа и bToken, валюта резерва и обе паузы. [Привязки и паузы](../{DATA/'baseline_base_components.json.gz'}). "
        f"[Выплаты BSR/cbBTC](../{DATA/'baseline_base_wallets.json.gz'}).",
        "", "У Omni Flux перечислен 71 NFT позиции. Их 38 уникальных ключей проверены через CLPoolManager.poolIdToPoolKey; "
        "на 11 ключах была активная ликвидность и у всех 38 адрес хука нулевой. Все 38 индивидуальных записей индексатора имеют ID omni-exchange-v4-base. В архиве исполнены 36 ног штатного Flux Router и восемь ног через V5. На девяти первых Bin-пулах измерены 21 нога штатного роутера и 15 ног V5. У части рынков проходят только единицы raw, в отдельных направлениях BinPool сообщает OutOfLiquidity. Результаты относятся к указанным объёмам и кошелькам. "
        f"[Роли Flux и записи CL PositionManager](../{DATA/'omni_flux_base_directory.json.gz'}) · [Выплаты CL](../{DATA/'omni_flux_cl_wallets.json.gz'}) · [Выплаты Bin и резервы перед каждой ногой](../{DATA/'omni_flux_bin_wallets.json.gz'}).",
        "", "Полный реестр Omni Flux от блока 32 873 799 до блока 62 262 756 содержит 38 Initialize CL и 12 Initialize Bin. "
        "Реестр CL совпал со всеми 38 физическими PoolId из 71 NFT позиции. Среди этих рынков 17 CL и 4 Bin имеют физическую валюту ETH и нулевые флаги хуков. "
        "В первом архиве для этих рынков на сыром ETH подтверждено 28 ног CL и 6 Bin через опубликованный Universal Router с точным SETTLE. По WETH получены по 28 и 6 прямых выплат модуля, а также по 28 и 6 выплат через модуль в V5. "
        "Для Bin официальный getBin может возвращать остаток вне дерева исполнимых бинов. На рынке ETH/USDC при переходе 8 382 111 → 8 382 125 пропущены 13 таких остатков. "
        "Профили omni-flux-native-cl-base и omni-flux-native-bin-base закрепляют менеджер, Vault, квотер и PoolId. Код разрешённого токена для прямой оплаты ETH указывается в dex.router_exact_tokens. "
        "Адрес своего модуля для WETH задаёт владелец в dex.router. Старый O1NativeAdapter воспроизводит исторический стандарт o1; для общего модуля o1, Omni CL и Omni Bin получено 2 227 924 gas в архиве 45934365428. "
        f"[Полный реестр](../{DATA/'omni_flux_registry.json.gz'}) · [ETH и WETH в CL](../{DATA/'base_native_cl_wallets.json.gz'}) · [ETH и WETH в Bin](../{DATA/'base_native_bin_wallets.json.gz'}) · [Точные реализации ERC20](../{DATA/'omni_flux_exact_tokens.json'}).",
        "", "Treble LimitOrderManager сообщил 274 эпохи, 230 с флагом filled и восемь с ненулевой ликвидностью без этого флага. "
        "Шесть базовых пулов подтверждаются фабрикой и индивидуальными записями индексатора Treble V4. Для мгновенной покупки и продажи используется соответствующий пул. "
        f"[Эпохи, рынки и фактические вызовы](../{DATA/'treble_limit_epochs.json.gz'}).",
        "", "DeJungle и Ensurer: поиск по названию дал ноль пулов. В открытом репозитории DeJungle есть README. Адрес продажи Ensurer на BSC не содержит кода на контрольном блоке Base. Для атрибуции Base нужны первичный адрес фабрики и связанный с ней рынок. "
        f"[Ответы каталогов и проверка адреса](../{DATA/'base_catalog_identity.json'}).",
        "", f"[Канонические вызовы и квитанция деплоя](../{DATA/'base_recovered_markets.json'}) · [Спецификация идентификатора bToken](https://www.baseline.markets/docs/contracts/api).",""]


def registry_row(network,id):
    if id in {"dex-on-crypto-base","dex-on-crypto-ethereum"}:
        r=json.loads((DATA/"docswap_empty_directories.json").read_bytes())[network]
        return None,r["factory"],"Router.factory и WETH; allPairsLength = 0","фабрика проверена, 0 зарегистрированных пар на снимке"
    if network!="base":return None
    if id in {"ryze","thirdfy","baseline-base"}:
        r=json.loads((DATA/"base_recovered_markets.json").read_bytes())
        if id=="thirdfy":
            m=r["thirdfy"]
            return m["markets"][0]["pool"]["address"],m["factory"]["address"],"poolByPair для 6/6 рынков","DEX-41 · 24 ноги V5 на шести рынках, динамическая комиссия BasePluginV1"
        if id=="ryze":
            m=r["ryze"]
            return m["markets"][0]["pool"]["address"],m["factory"]["address"],"factory.pools и Router.factory","DEX-41 · для собственной выплаты требуется подписанный ордер, nonce и данные оракула"
        b=r["baseline"]
        return b["pool_id"],b["initialize"]["address"],"квитанция deploy → Initialize → ключ","DEX-41 · BSR/cbBTC, 8 выплат Router/V5; профиль Mercury сверяет действующие компоненты и их селекторы"
    if id=="omni-exchange-v4-base":
        d=evidence("omni_flux_base_directory.json.gz")
        return d["markets"][0]["id"],d["roles"]["clPoolManager"],"Flux Router, NFT и канонические PoolKey","DEX-41 · 38/38 ключей CL отнесены к ID, 36 штатных выплат и 8 V5 на проверенных объёмах"
    if id=="treble-v4-limit":
        d=evidence("treble_limit_epochs.json.gz")
        return d["markets"][0]["pool"],d["factory"],"274 epochInfos → 6 базовых пулов Treble V4","DEX-41 · лимитные заявки на существующих рынках, исполнение зависит от пересечения диапазона"
    if id in ("dejungle","ensurer"):
        return None,None,"проверены поиск по названию и опубликованные проектные материалы","DEX-41 · недостаточно исходных данных для привязки фабрики и рынка Base"
    if id=="marswap-base":
        with gzip.open(DATA/"marswap-base.json.gz") as f:d=json.load(f)
        return None,d["factory"],"allPairsLength = 0 и INIT_CODE_PAIR_HASH","фабрика проверена на Base; 0 зарегистрированных пар на снимке"
    if id in ("bankr","bankr-index-base","o1-launchpad"):
        d=evidence("base_launch_hooks.json.gz");m=next(r for r in d["rows"] if r["group"]==id and "terms" in r)
        return m["id"],d["manager"],"Initialize, keccak PoolKey и регистрация на хуке","DEX-40 · публичные кошельковые выплаты по каждой выборке приведены выше"
    return None
