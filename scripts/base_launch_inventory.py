"""Сводит адреса и результаты запусков Bankr и o1 из канонических архивов без сетевых чтений."""
import gzip
import json
from pathlib import Path

DATA=Path("tests/data/first_run_dex_2026-11-26")

def evidence(name):
    opener=gzip.open if name.endswith(".gz") else open
    with opener(DATA/name,"rt",encoding="utf8") as f:return json.load(f)

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
        "Три включённых в цепь заявки Ryze сверены по подписи EIP-712, смене пользовательского nonce и событиям ERC-20. В двух обменах USDC/WETH списание равно объёму заявки. В третьем обмене кошелёк перевёл 3476 атомов cbBTC, получил отдельно 1 атом cbBTC от подписанного пула и 2053827 атомов USDC от роутера. Фактический расход cbBTC составил 3475 атомов. [Хеши, комиссии и платежи в трёх квитанциях](../tests/data/first_run_dex_2026-11-26/ryze_base_verified_intents.json.gz).",
        "На Base с блока 51 000 000 по 60 966 755 зафиксированы 2 обмена WETH→cbBTC, 2 cbBTC→WETH и 26 USDC→cbBTC. Для четырёх двухшаговых и двух одношаговых обменов квитанции подтвердили расход и выплату кошелька. Во всех четырёх двухшаговых комиссия равнялась удвоенной округлённой вверх плате за шаг. Округление после умножения занизило бы каждую комиссию на один атом входного токена. [Исходные заявки, квитанции и комиссия родительского блока](../tests/data/first_run_dex_2026-11-26/ryze_base_path_history.json.gz).",
        *ryze_settings(),"",f"Фабрика Thirdfy Algebra Integral `{third['factory']['address']}` подтвердила каждый из шести рынков. "
        "На каноническом чтении все шесть имели ненулевую активную ликвидность и pluginConfig 193. Комиссия на высоте варьировалась от 3000 до 7386 миллионных. ArbExecutor V5 исполнил 24 ноги на шести рынках, в обоих направлениях и на двух объёмах. "
        f"[Архив кошельковых выплат и повышенных минимумов](../{DATA/'thirdfy_base_wallets.json.gz' }).",
        "",f"Baseline использует bToken `{bl['btoken']['address']}` в качестве идентификатора страницы. Реальный ключ BSR/cbBTC `{bl['pool_id']}` "
        f"создан в транзакции `{bl['index']['createdAtTxnId']}`, хук `{bl['hook']['address']}`. "
        "На 100 и 1000 raw cbBTC владелец получил 26019463245970438 и 260194412075800528 raw BSR. "
        "При продаже этих объёмов вернулись 98 и 980 raw cbBTC. Роутер и V5 дали одинаковый чистый выход во всех восьми ногах. "
        f"Реестр Relay вернул семь компонентов, 113 распределённых ABI-селекторов и разрешённый пул. При расчёте вновь сверяются весь реестр, хеши, маршруты, обратимое соответствие ключа и bToken, валюта резерва и обе паузы. [Привязки и паузы](../{DATA/'baseline_base_components.json.gz'}). "
        f"[Выплаты BSR/cbBTC](../{DATA/'baseline_base_wallets.json.gz'}).",
        "", "У Omni Flux перечислен 71 NFT позиции. Их 38 уникальных ключей проверены через CLPoolManager.poolIdToPoolKey; "
        "на 11 ключах была активная ликвидность и у всех 38 адрес хука нулевой. Все 38 индивидуальных записей индексатора имеют ID omni-exchange-v4-base. В архиве исполнены 36 ног штатного Flux Router и восемь ног через V5. На 12 зарегистрированных Bin-пулах собраны 26 положительных ног штатного роутера и 20 ног V5. У части рынков проходят только единицы raw, в отдельных направлениях BinPool сообщает OutOfLiquidity. Результаты относятся к указанным объёмам и кошелькам. "
        f"[Роли Flux и записи CL PositionManager](../{DATA/'omni_flux_base_directory.json.gz'}) · [Выплаты CL](../{DATA/'omni_flux_cl_wallets.json.gz'}) · [Выплаты Bin и резервы перед каждой ногой](../{DATA/'omni_flux_bin_wallets.json.gz'}).",
        "", "Полный реестр Omni Flux от блока 32 873 799 до блока 62 262 756 содержит 38 Initialize CL и 12 Initialize Bin. "
        "Реестр CL совпал со всеми 38 физическими PoolId из 71 NFT позиции. Среди этих рынков 17 CL и 4 Bin имеют физическую валюту ETH и нулевые флаги хуков. "
        "В первом архиве для этих рынков на сыром ETH подтверждено 28 ног CL и 6 Bin через опубликованный Universal Router с точным SETTLE. По WETH получены по 28 и 6 прямых выплат модуля, а также по 28 и 6 выплат через модуль в V5. "
        "Для Bin официальный getBin может возвращать остаток вне дерева исполнимых бинов. На рынке ETH/USDC при переходе 8 382 111 → 8 382 125 пропущены 13 таких остатков. "
        "Профили omni-flux-native-cl-base и omni-flux-native-bin-base закрепляют менеджер, Vault, квотер и PoolId. Код разрешённого токена для прямой оплаты ETH указывается в dex.router_exact_tokens. "
        "Для обеих физических валют владелец задаёт свой адрес в dex.router. Адрес размещения отсутствует в первичной записи. Старый O1NativeAdapter воспроизводит прежнюю проверку o1 WETH. На GitHub 45935873159 основной BaseNativeMarkets дал runtime 0x10ef3275e543bf04b01d5d4e17e4fd1391f12aebc1a708b8e24ddb0b580aac01 с восемью проверенными immutable-адресами и затратой 2 644 660 gas на создание. Нативный executeNative подтвердил ещё 28 ног CL (151 733–2 373 401 gas) и 6 Bin (170 976–331 116 gas). Поручения с отличающимся msg.value, другим PoolId, просроченным сроком и увеличенным минимумом отклонены. Хеш прежней общей сборки 0x397dff8706a95fe3078d9a0dbca7b46e6825a1a67a09f153ac2efe8e7581b024 применим к проверенным путям WETH. Для выбора физического ETH проверяется dex.infinity_native_module_hash. "
        f"[Полный реестр](../{DATA/'omni_flux_registry.json.gz'}) · [ETH и WETH в CL](../{DATA/'base_native_cl_wallets.json.gz'}) · [ETH и WETH в Bin](../{DATA/'base_native_bin_wallets.json.gz'}) · [Точные реализации ERC20](../{DATA/'omni_flux_exact_tokens.json'}).",
        "", "Treble LimitOrderManager сообщил 274 эпохи, 230 с флагом filled и восемь с ненулевой ликвидностью без этого флага. "
        "Шесть базовых пулов подтверждаются фабрикой и индивидуальными записями индексатора Treble V4. На пяти из шести пулов две покупки и две полные продажи на двух объёмах подтвердились движением средств кошелька (20 выплат). У шестого пула USDC/cbBTC нулевая активная ликвидность, на обоих объёмах полная покупка не исполнилась. Профиль treble-v4 обслуживает эти рынки один раз. В пяти исторических транзакциях Fill отдельно сверены соседние блоки, Swap и зачисления. Четыре эпохи принесли токены за сожжённую ликвидность. В пятой, №5, до пересечения уже было ноль ликвидности, зачислено ноль. "
        f"[Эпохи и рынки](../{DATA/'treble_limit_epochs.json.gz'}) · [Суммы кошелька и откаты](../{DATA/'treble_limit_wallets.json.gz'}) · [Исторические квитанции и суммы эпох](../{DATA/'treble_limit_settlements.json.gz'}).",
        "", "DeJungle и Ensurer: поиск по названию дал ноль пулов. В открытом репозитории DeJungle есть README. Адрес продажи Ensurer на BSC не содержит кода на контрольном блоке Base. Для атрибуции Base нужны первичный адрес фабрики и связанный с ней рынок. "
        f"[Ответы каталогов и проверка адреса](../{DATA/'base_catalog_identity.json'}).",
        "", f"[Канонические вызовы и квитанция деплоя](../{DATA/'base_recovered_markets.json'}) · [Спецификация идентификатора bToken](https://www.baseline.markets/docs/contracts/api).",""]


def ryze_settings():
    import tomllib
    rec=tomllib.loads(Path("config/first_run.toml").read_text(encoding="utf-8"))
    rows=[r for section in rec["section"] for r in section.get("rec",[]) if r["name"].startswith("ryze.")]
    lines=["","#### Спотовые заявки Ryze","","Вход каждой заявки представляет полное разрешённое списание. После перевода фиксированной USD-платы в атомы входного актива с округлением вверх адаптер умножает её на число пулов. Вторую персональную котировку запрашивает на оставшуюся сумму. Подпись закрепляет всю последовательность пулов, адрес получателя и минимум сделки. Газ batch оплачивает отправитель релея.","",
           "| Настройка Base | Значение | Источник |","| --- | --- | --- |"]
    for r in rows:
        source="[Архив токенов](../tests/data/first_run_dex_2026-11-26/ryze_base_tokens.json)" if r["name"]=="ryze.token_bindings" else (f"[Ryze]({r['source']})" if r["source"].startswith("https://documentation.ryze.pro/") else "[Канонический реестр](../tests/data/first_run_dex_2026-11-26/base_recovered_markets.json)")
        lines.append(f"| `{r['name']}` | "+", ".join(f"`{x}`" for x in r["value"].split(","))+f" | {source} |")
    lines.extend(["","Владелец задаёт разрешение `ryze.trading`, допустимый возраст ответа `ryze.quote_age_ms`, срок подписи `ryze.validity_sec`, допуск в базисных пунктах `ryze.slippage_bps`, время ожидания цены `ryze.quote_budget_ms`, интервал фоновой книги `ryze.books_interval_ms` и размеры `ryze.quote_sizes` в форме адрес=количество торгуемого токена. Суммы актива и разрешение ERC20 на роутер проверяются у выбранного кошелька в каноническом блоке.","",
       "WETH9 переводит точную сумму. Действующие реализации USDC и cbBTC уменьшают и увеличивают балансы на одинаковый value, учитывая pause, blacklist и allowance. Администраторы ZeppelinOS могут сменить реализацию после выдачи подписи. Код и слоты проверяются перед отправкой и на блоке выплаты. Обновление в этом блоке требует отдельной сверки. Перед первым POST журнал сохраняет digest и публичные параметры; восстановление использует каноническую квитанцию, полный расход и чистые переводы кошелька. [Исходники и роли подкреплённых контрактов](../tests/data/first_run_dex_2026-11-26/ryze_base_tokens.json).",""])
    return lines

def registry_row(network,id):
    if id in {"dex-on-crypto-base","dex-on-crypto-ethereum"}:
        r=json.loads((DATA/"docswap_empty_directories.json").read_bytes())[network]
        return None,r["factory"],"Router.factory и WETH; allPairsLength = 0","В исходной сети 0 пар. В изолированной симуляции фабрика создала пару, штатный Router и V5 провели 8 обменов, завышенный минимум и истёкший срок дали откат. Профиль проверяет комиссию фабрики при появлении ликвидности"
    if network!="base":return None
    if id in {"ryze","thirdfy","baseline-base"}:
        r=json.loads((DATA/"base_recovered_markets.json").read_bytes())
        if id=="thirdfy":
            m=r["thirdfy"]
            return m["markets"][0]["pool"]["address"],m["factory"]["address"],"poolByPair для 6/6 рынков","DEX-41 · 24 ноги V5 на шести рынках, динамическая комиссия BasePluginV1"
        if id=="ryze":
            m=r["ryze"]
            return m["markets"][0]["pool"]["address"],m["factory"]["address"],"factory.pools и Router.factory","RYZE-RECON · две спотовые пары; персональная заявка EIP-712, собственный nonce роутера и сверка чистых выплат, 9 архивных сделок"
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
        return None,d["factory"],"allPairsLength=0; INIT_CODE_PAIR_HASH, factory/WETH роутера подтверждены","DEX-41 · профиль первого запуска; на оригинальной пустой фабрике 12 выплат в изолированном состоянии, на контрольном блоке 0 пар"
    if id in ("bankr","bankr-index-base","o1-launchpad"):
        d=evidence("base_launch_hooks.json.gz");m=next(r for r in d["rows"] if r["group"]==id and "terms" in r)
        return m["id"],d["manager"],"Initialize, keccak PoolKey и регистрация на хуке","DEX-40 · публичные кошельковые выплаты по каждой выборке приведены выше"
    return None


def spectra():
    d=evidence("spectra_base_directory.json.gz")
    active=sum(r["maturity"]>d["timestamp"] for r in d["markets"])
    both=sum(all(v for k,v in r.items() if k.startswith("quote_")) for r in d["markets"])
    wallets=evidence("spectra_base_wallets.json.gz")
    measured=[(m,w,l) for m in wallets["markets"] for w in m["wallets"] for l in w.get("legs",[]) if l["stage"]=="accepted"]
    modes={mode:sum(w["mode"]==mode for _,w,_ in measured) for mode in ("pool","v5","router")}
    markets={m["pool"] for m,_,_ in measured}; mature={m["pool"] for m,_,_ in measured if m["maturity"]<=d["timestamp"]}
    return ["","#### Spectra V2 на Base","",
            f"На блоке {d['block']} реестр Spectra хранит {len(d['markets'])} PT. Две фабрики CryptoSwap V1 перечисляют 24 и 22 пула, а StableSwap NG — 42. "
            f"Четыре положительных get_dy (обе стороны и два размера) получены на {both} пулах. У {active} PT срок погашения ещё впереди. Завершение срока меняет условия погашения PT, а возможность продажи IBT/PT определяется текущими резервами и фактической передачей токенов. "
            f"[Каждый пул, его PT, IBT, underlying, YT, срок, резервы и котировки](../{DATA/'spectra_base_directory.json.gz'}).","",
            "У обоих шаблонов StableSwap NG версия v7.0.0. Первые семь пулов отдают администратору 50% комиссии, остальные 35 — 80%. "
            "Обмен CryptoSwap совпадает с исходником Curve; у второй фабрики есть дополнительная операция donate для администратора. Граф рассчитывает каждую из этих кривых один раз на физический пул. "
            "Профиль читает цену двух фактических монет IBT и PT. Для отдельного погашения в underlying понадобятся условия конкретного ERC4626, а до срока также встречные YT. "
            f"[Разница проверенных исходников и хеши исполнения](../{DATA/'spectra_base_sources.json'}).","",
            f"Архив оплат содержит {modes['pool']} исполненных направлений напрямую через пул и {modes['v5']} через V5. "
            f"Среди {len(markets)} подтверждённых рынков у {len(mature)} срок PT уже истёк. "
            + (f"Через штатный Spectra Router исполнено {modes['router']} ног. " if modes["router"] else "")
            + "Штатный Router ограничивает абсолютный баланс получателя после выплаты. V5 проверяет прирост баланса за саму транзакцию и её срок. Для исполнения профиля нужны средства в физической монете и заданный владельцем проверенный V5. "
            f"[Входы, выходы, газ и контрольные отказы](../{DATA/'spectra_base_wallets.json.gz'}).",""]

def umia():
    d=evidence("umia_base_directory.json.gz")
    h=evidence("umia_base_history.json.gz")
    w=evidence("umia_base_wallets.json.gz")
    counts={mode:sum(1 for m in w["markets"] for c in m["cases"] if c["mode"]==mode
                     for side in ("buy","sell") if (c.get(side) or {}).get("stage")=="accepted")
            for mode in ("router","v5")}
    return ["#### Umia · спотовые vault и миграция","",
        f"Hub на высоте {d['height']} вернул семь venture, четыре текущих спотовых vault и три venture без vault. "
        f"События Hub от {h['first']} до {h['last']} включают ещё один пул UMIA/USDC с комиссией 1%, "
        f"чей предыдущий vault сохранил ликвидность {h['retired'][0]['liquidity']}. У нового пула ставка 0,3%. "
        "Из трёх отдельных Koi3 TEST/USDC (1%) два имели нулевую ликвидность в текущем ценовом диапазоне. "
        "[Реестр venture и ключи](../tests/data/first_run_dex_2026-11-26/umia_base_directory.json.gz). "
        "[Каноническая история выдачи и миграции](../tests/data/first_run_dex_2026-11-26/umia_base_history.json.gz).","",
        f"В архивной симуляции обычный Universal Router провёл {counts['router']} обменов, V5 — {counts['v5']}. "
        "Списания входа и чистые выплаты измерены по балансу кошелька, проверены минимальная выплата +1 и истекший срок. "
        "Профиль берёт из настроек проверенные хеши хуков и фабрик, канонический ключ, роль зарегистрированного LBP либо мигратора. "
        "Предыдущий vault проверяется по истории, принятому runtime и обратным связям. В GitHub сверены 9481 байт исполняемого кода UmiaHook и 12392 байта UmiaHookV2; различаются метаданные сборки. Ранний hook у двух Koi остался без совпавшего исходника. "
        "[Суммы и газ](../tests/data/first_run_dex_2026-11-26/umia_base_wallets.json.gz). [Сравнение исходных версий](../tests/data/first_run_dex_2026-11-26/umia_base_sources.json).","",
        "Статистика DefiLlama объединяет спотовые и условные рынки. Обмен внутри UmiaMarketCore ведёт права на исход предложения "
        "в собственном реестре. Выдача обеспечения зависит от погашения прав и результата settlement. "
        "[Первичные контракты](https://github.com/umiafinance/protocol/tree/3165f5a5bdaa23a83fbfd4894523e7ca84d53a14).",""]

def fynd(network):
    f=evidence("fynd_"+network+"_markets.json.gz")
    markets=f["markets"]
    shared=sorted({r["controller"] for r in markets})
    listing=", ".join(f"`{r['id']}` ({r['tokens'][0]} / {r['tokens'][1]})" for r in markets)
    return ["#### Fynd AMM · подписанная комиссия","",
        f"Для этой площадки задайте `fynd.base_url = https://fynd-api.propellerheads.xyz/v1` и `fynd.network = {network}`. Для `fynd.pools` укажите строки `PoolId=token0=token1` из следующего списка. Значения также доступны в рекомендациях первого запуска.", "",
        f"На высоте {f['height']} проверены все {f['initialize_logs']} событий PoolInitialized ядра Ekubo с момента размещения расширения. "
        f"Расширения Fynd создали {len(markets)} физических ключей. {listing}.","",
        f"Контроллер на этом блоке {', '.join(shared)}. Для каждого ключа вызов beforeSwap вернул SwapMustHappenThroughForward. "
        "Ключ Ekubo задаёт базовую ставку 0. Для исполнения контроллер подписывает PoolId, предел изменения баланса, плату за конкретный обмен, срок, nonce и ограничения locker. "
        "Нулевое поле authorizedLocker допускает любой locker, иначе контракт сравнивает младшие 128 бит. Продукт указывает Tycho Router. "
        "Проверка минимального изменения баланса внутри Core предшествует отдельной подписанной плате. "
        f"Публичная проверка hosted API без ключа вернула HTTP {f['unauthenticated_health']['status']}. Для торгового допуска необходимы ключ участника, "
        "подключение Fynd AMM routing, живая подпись для собственного допустимого маршрута и архивный замер чистой выплаты. "
        f"[Канонические контракты, PoolId и контрольные отказы](../tests/data/first_run_dex_2026-11-26/fynd_{network}_markets.json.gz). "
        "[Правила Fynd](https://docs.fynd.xyz/get-started/fynd-amm.md). [Получение ключа и условия интеграции](https://docs.fynd.xyz/get-started/hosted-api.md).","",
        "Публичный интерфейс swap.fynd.xyz предлагает пользователю маршруты со своей подписью Fynd, если AMM выигрывает по цене. Владелец может получить пользовательскую котировку через этот интерфейс. Для автоматизированной интеграции требуется разрешённый канал выдачи свежих подписей для его маршрута. У опубликованной модели комиссия равна 90% преимущества совокупного маршрута после газа или 5 bps при отсутствии публичного маршрута сравнения. Она удерживается только с ноги AMM. [Публичный обмен](https://swap.fynd.xyz/). [Правила AMM](https://docs.fynd.xyz/get-started/fynd-amm.md).","",
        "Лицензия Fynd 1.0 требует проводить операции на основе его котировки через действующий Designated Router сети. Пункт 4.2 исключает доходность торговли собственным капиталом из базы Revenue Share. Для выручки сервиса и вознаграждения solver предусмотрена отдельная доля 20%. Газ, фактическая плата AMM и сборы роутера учитываются в чистой цене. [Разделы 1.11–1.15, 3 и 4 лицензии](https://github.com/propeller-heads/fynd/blob/8839d44f4b482ac480daf122a41ddb59ad91dae8/LICENSE.md).","",
        "В Telegram владелец может получить через /start один ключ для hosted Fynd и индексатора Tycho. Для доступа к подписанным свопам Fynd отдельно включает интегратору AMM routing. API 0.118.4 принимает точный вход и фильтры исключения пулов, протоколов и токенов. Для строки Fynd AMM каждый шаг ответа должен ссылаться на подтверждённый PoolId этой сети; проверяются также исполняемые calldata, получатель, срок, плата и фактическая выплата. Смешанные маршруты учитываются по отдельным шагам. [Схемы QuoteRequest, RouteFilter и Swap](https://docs.fynd.xyz/reference/api.md).","",
        "Ручная проверка в карточке кошелька на Base и Ethereum использует секрет `fynd.api_key`, выданный владельцу в Hosted API. Введите адреса и целую сумму в минимальных единицах. Бот проверяет chain_id и сверяет номер, время и хеш блока с узлом. В ответе отображается число шагов через зарегистрированные собственные пулы Fynd. Ручной запрос цены не подписывает поручение. Сервер может вернуть маршрут других протоколов при действующем ключе.", "",
        "Отдельная схема InBlock (JIT) сохраняет исходный маршрут при отсутствии замены. Для включения своего роутера команда Fynd согласует условия и регистрирует декодер IJITDecoder; плательщик одобряет FyndJITRouter и указывает конечного получателя. В опубликованном реестре JIT указан Ethereum Mainnet, 0x7e67f82476bb4bd4c5078F63e5a588dFD9CDD91E. Реализация на Base и действующая персональная котировка по одному Fynd AMM здесь остаются неподтверждёнными. [Регистрация агрегатора](https://docs.fynd.xyz/get-started/fynd-in-block-solver/fynd-in-block-for-dex-aggregators.md). [Размещения InBlock](https://docs.fynd.xyz/get-started/fynd-in-block-solver.md).",""]


def vecta():
    d=evidence("vecta_base_directory.json")
    t=evidence("vecta_base_tiers.json.gz")
    w=evidence("vecta_base_wallets.json.gz")
    return ["#### VectaSwap · кошелёк и фабрика","",
        f"На высоте {d['block']} nonce фабрики {d['nonce']} подтвердил все три PoolCreated. Совпали CREATE2 и исполняемый runtime "
        "каждого пула с эталоном Uniswap V3 после учёта объявленных immutable. Код и роли SwapRouter, QuoterV2, TickLens проверены у размещений "
        f"того же отправителя. Уровни комиссии в миллионных долях и шаг тика: {', '.join(f'{k} → {v}' for k,v in sorted(t['tiers'].items(),key=lambda x:int(x[0])))}. "
        "[Реестр и привязки байткода](../tests/data/first_run_dex_2026-11-26/vecta_base_directory.json). "
        "[Все изменения разрешённых ставок](../tests/data/first_run_dex_2026-11-26/vecta_base_tiers.json.gz).","",
        "EURC/USDC: при двух расходах 1 000 и 1 000 000 raw USDC кошелёк получил 910 и 911 093 raw EURC. "
        "При обратном обмене тем же маршрутом получено 997 и 999 798 raw USDC. Штатный Router и V5 дали равные суммы. "
        "Для WETH/USDC и WETH/USDT на этой высоте нулевая активная ликвидность и два отказа котировки. "
        "Обе монеты действующей пары допускают смену реализации, поэтому обычный Router с номинальным минимумом не подтверждает "
        "прирост баланса для произвольной реализации. Для заданного владельцем адреса dex.v3_wallet_module профиль сверяет runtime и фабрику. "
        "Модуль контролирует полный вход и чистую выплату. На 1 USDC покупка стоила 150 300 gas против 163 801 в V5; "
        "обратная продажа — 128 920 против 142 483 gas. Реальный адрес нового модуля задаёт владелец при его размещении. "
        "[Списания, выплаты, газ и контрольные отказы](../tests/data/first_run_dex_2026-11-26/vecta_base_wallets.json.gz).",""]

def spoke(network):
    out=[]
    for name,title,slug in (("maverick","Maverick V1","maverick-v1"),("topaz","Topaz CL","topaz-cl")):
        stem=f"{name}_v1_{network}"
        d=evidence(stem+"_directory.json.gz"); w=evidence(stem+"_wallets.json.gz")
        samples=w["samples"]
        ok={m["pool"] for m in samples for c in m["cases"] for side in ("buy","sell") if c.get(side) and c[side]["stage"]=="paid"}
        counts={}
        for m in samples:
            for c in m["cases"]:
                counts[c["wire"]]=counts.get(c["wire"],0)+sum(bool(c.get(k)) and c[k]["stage"]=="paid" for k in ("buy","sell"))
        roles=", ".join(f"{k} `{v['address']}`" for k,v in d["roles"].items())
        out.extend([f"#### {title}","",f"На высоте {d['height']} фабрика зарегистрировала {len(d['markets'])} пулов. Проверено {len(samples)}, кошельковая симуляция показала выплату у {len(ok)} пулов. {roles}.",""])
        label={"router":"официальный роутер","module":"модуль кошелька","module_v5":"модуль через V5","v5":"V5"}
        out.append("Выплаченные операции по обеим сторонам: "+", ".join(f"{label[k]} {v}" for k,v in counts.items())+". Каждый такой обмен прошёл при минимуме, равном чистому зачислению; минимум на один атом выше и просроченный вызов завершились отказом.")
        if name=="maverick":
            out.extend(["",f"Модуль MaverickV1Adapter привязан к фабрике этой сети. Архивный runtime `{w['module_hash']}`. Владелец задаёт фактический dex.router после размещения модуля и переключает dex.kind с maverick_v1 на maverick_v1_module. Официальный PoolInformation на точном входе возвращает только выход. Модуль проверяет полное списание, callback зарегистрированной пары и реальное зачисление. Владелец задаёт размеры лестницы dex.pmm_ladder_usd согласно своему бюджету."])
        else:
            out.extend(["","Плата пула fee() учитывает отправителя и состояние TWAP в блоке. Фабрика поддерживает запасную ставку на случай отказа модуля. Цена перед отправкой проверяется от своего адреса. Повторный шаг по тому же рынку требует нового состояния модуля."])
            fee=evidence(f"topaz_{network}_fees.json.gz")
            cur=fee["blocks"][1]
            rates=sorted({v["value"][0] for m in cur["markets"] for v in m["fees"].values()})
            out.extend(["",f"На трёх соседних высотах проверены все {len(cur['markets'])} пулов. Окно TWAP 600 секунд, масштаб 1 000 000, предел по умолчанию 50 000, коэффициент по умолчанию 0. На центральной высоте фактические ставки в миллионных долях: {', '.join(map(str,rates))}. Событий регистрации и снятия скидки от блока {fee['from_block']} до {fee['to_block']} включительно: {len(fee['registrations'])}. [Ставки от кошельков и параметры каждого пула](../tests/data/first_run_dex_2026-11-26/topaz_{network}_fees.json.gz)."])
        out.extend([f"[Реестр и размещения](../tests/data/first_run_dex_2026-11-26/{stem}_directory.json.gz). [Канонические симуляции](../tests/data/first_run_dex_2026-11-26/{stem}_wallets.json.gz). [Документация контрактов]({d['source']}).",""])
    return out
