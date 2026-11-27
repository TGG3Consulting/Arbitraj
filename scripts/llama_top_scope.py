"""Методика привязки: ID DefiLlama, адресный профиль, фактический источник и граница коммерческого допуска."""

# empty profile covers configured external gateways, access-controlled venues and non-DEX statistics.
# Each line range references the immutable adapter revision in the source snapshot.
def rows(net):
    base = net == "base"
    entries = {}
    def add(id, kind, profiles, lines, boundary, proof=None):
        entries[str(id)] = dict(kind=kind, profiles=profiles.split(), lines=lines,
                                 boundary=boundary, evidence=proof or f"{net}.json")
    add(8691,"factory",f"aero-slipstream-{net}", ("29" if base else "32")+",39-40,46-50,100-122,176-179",
        "MetaDEX регистрирует пары через PoolCreated и учитывает их Swap. Старый Lite в этой методике применяется исключительно на Arc.",f"metadex_{net}.json")
    add(2198,"dataset","uniswap-v3-base" if base else "uniswap_v3","76-91,161-177,195-208,"+("329" if base else "322"),
        "Dune dex.trades отбирает project=uniswap/version=3 по сети. У контрольного эмитента фабрика и обратная регистрация совпадают. Строки полного суточного расчёта Dune отдельно не выгружены.","llama_top_contracts.json.gz")
    add(5690,"manager",f"uniswap-v4-{net}",("116-120" if base else "92-96")+",86-88,369-395",
        "Swap и ModifyLiquidity общего PoolManager. Поручение выбирает конкретный PoolId и разрешённый hook; общий оборот включает и другие ключи.","llama_top_contracts.json.gz")
    add(5317,"registry",f"fluid-{net}",("38-39,55-56" if base else "32-33,49-50")+",64-67,84-102",
        "Swap учитывается у рынков getAllPools(). Множество resolver и настроенной фабрики совпало на контрольном блоке.","llama_top_contracts.json.gz")
    add(8306,"factory",f"metric-{net}","37-59,73-75",
        "PoolCreated и Swap у действующей фабрики. Учёт прежней фабрики завершился 22 сентября 2026 года.",f"metric_{net}.json")
    if base:
        add(2769,"factory","pancakeswap-v3-base","53-56,106-114,138-148,199-205,265-270",
            "Фабрика регистрирует V3-пулы. В topic Swap Pancake включены обе protocolFees; у контрольного пула проверена обратная регистрация.","llama_top_contracts.json.gz")
        add(4524,"factory","aerodrome-slipstream aerodrome-slipstream-2 aerodrome-slipstream-3 aerodrome-slipstream-early","17-38,50-52",
            "Четыре фабрики совпали с четырьмя профилями. У ранней 0x9592… 22 зарегистрированные пары. В 24 архивных ногах V5 и Universal Router проверены две пары, в четырёх случаях вход исполнен частично.","llama_top_contracts.json.gz")
        add(5919,"partial","","6-13,46-55,117-132,152-160","Три подтверждённые книги источника имеют другие адреса, чем восемь из API. За 8192 блока у трёх 44 агрессивных исполнения. В контрольной выплате одна из восьми касс приняла cbBTC и перечислила WETH через книгу источника; повторный учёт этого запаса исключён.","llama_top_hanji.json.gz")
        add(7984,"conditional","","6-31,48-58","Три исторических эмитента и исключения получателей заданы раздельно. Для текущей реализации собственное исполнение требует права на подписанные котировки.","kipseli_base.json")
        add(9172,"factory","aero-stable-base aero-volatile-base","15-24,32-34,52-66","Две фабрики Basic порождают отдельные пары. Объём собирается по Swap каждого физического пула.","aero_basic_base.json")
        add(3450,"factory","aerodrome-base","15-20,26-29,99-111,171-184","У Classic своя фабрика; PoolCreated выбирает пары, затем считаются Swap. GaugeFactory служит для учёта вознаграждений.")
        add(7260,"router","elfomofi","6-9,27-33,125-129","ElfomoTrade одного контракта. VaultMetrics измеряет другие показатели. Валидные размеры и активы проверяются для кошелька.","elfomofi_base.json")
        add(3481,"factory","solidly-v3-base","813-818","Отдельная Base factory Solidly V3. Кошельковое исполнение закрепляет физический пул и проверяет полный вход.","solidly_v3_base.json")
        add(8180,"card","","117-128","Rain учитывает карточные расходы Karta через collateralProxy. Обменная пара и запас для спотовой сделки этой записью не задаются.")
        add(7601,"partial","lunarbase","14-39,61-64,100-110","В Base секции несколько поколений. С действующим профилем совпадает один native ETH/USDC. Архивы отдельных размещений и основной эмитент различаются.","lunarbase.json")
        add(8934,"router","baibai","7-19,28-34","Две версии Fill одного EntryPoint. Пул и право конкретного пользователя проверяются по отдельному состоянию рынка.","baibai_base.json")
        add(1461,"aggregate","woofi-swap","10-15,47-61","API /stat возвращает volume_usd для всей Base без адреса пула. Действующий WooPPV2 имеет отдельное доказательство; разбивка агрегата по кассам отсутствует.","woofi_base.json")
        add(8849,"shared","","4-16,24-29","Homelander считает обмены сторонних пулов с плагином, отдельно исключает собственные арбитражные ноги. Измерены пулы Hydrex и QuickSwap; нового запаса здесь нет.","homelander_base.json")
        add(6365,"factory","hydrex-integral","212-217","Фабрика Algebra и регистрация реальных пулов совпадают. Котировка учитывает актуальную роль динамической комиссии.","hydrex_base.json")
        add(2776,"factory","sushiswap-v3-base","7-12,71-80","Адрес фабрики Sushi из сетевой записи совпал с pool.factory и getPool контрольного пула.","llama_top_contracts.json.gz")
    else:
        add(3,"aggregate","curve curve-plain curve-twocrypto-ng curve-tricrypto-ng","11-53,585-599,607-624",
            "Первичный расчёт использует trading_volume сети Curve API. Для резервной ветви перечислены фабрики. Все пулы суточного агрегата и их доли не установлены.","curve_ethereum.json")
        add(7979,"conditional","","7-27","Методика учитывает три эмитента Fermi. Для подписанной операции нужны активный слот и доступная площадка на данном блоке.","fermi_ethereum.json")
        add(9436,"conditional","","5-15","Swapped выделенного контракта Stelaxis. На контрольной высоте все десять направлений пяти пар были неактивны.","stelaxis_ethereum.json")
        add(382,"partial","pendle","18-29,48-55,65-73,82-89","Вместе учитываются AMM Swap PT/SY и OrderFilledV2 лимитного роутера. Регистрация AMM не покрывает отдельные лимитные заявки.","pendle_ethereum_registry.json.gz")
        add(2803,"rfq","","10-22,25-36","RFQTrade собирается с noTarget по сети. Публичный Native API и адрес реальной кассы проверены отдельно; для полного объёма нужна атрибуция каждого эмитента.","native_rfq_ethereum_public.json")
        add(7599,"partial","lista-dex-v3 lista-dex-stableswap","58-65,82-99,105-113,178-185","Учитываются отдельные V3 PoolCreated/Swap и stable TokenExchange из smartCollateralConfig. Допуск каждой пары зависит от состояния контракта.","lista_ethereum.json")
        add(9435,"router","goliath-lambda-ethereum","5-14","Swapped выделенной кассы Goliath. Метка swap через агрегатор не заменяет выплату этой кассы.","goliath_ethereum.json")
        add(6650,"issuance","","5-13,23-34,76-79","TradeExecuted с attestationId и userId оценивает погашение и выпуск токенизированных бумаг. Обычный открытый спотовый пул этими данными не установлен.")
        add(2197,"factory","uniswap_v2","20-25,193-203,231-240","Фабрика V2 задаёт пары, учёт ведётся по их Swap при фильтрах методики.")
        add(7984,"partial","kipseli-lambda-ethereum","6-15,48-60","Эмитент 0x054f… связан с кассой и публичным входом LambdaClass. Сверены три прямые выплаты Kipseli одному EOA. Резервное исполнение Uniswap и окно доступности котировки учитываются отдельно.","kipseli_ethereum.json.gz")
        add(4222,"factory","ring-exchange-ethereum","13-19,40-52,78-86","Ring allPairs/i перечисляет пары. Swap учитывается с переводом единиц Few wrapped assets, роли обёрток проверены в торговом профиле.")
        add(8531,"router","tempest-lambda-ethereum","5-15","Swapped выделенной кассы Tempest. Чистая выплата именно Tempest отделена от резервной маршрутизации Uniswap.","propamm_router_ethereum.json.gz")
        add(9157,"factory","communityv3","745-750,955-960","Фабрика CommunityV3 совпадает с реестром. Выплата и пределы пула подтверждены кошельковым архивом.","community_v3_ethereum_wallets.json.gz")
        add(3499,"aggregate","ekubo-v2-ethereum ekubo-v3-ethereum","8-12,29-46","API Ekubo отдаёт volume по chainId и токену без физического core и PoolId. Есть проверенные торговые профили V2/V3; общая дневная сумма на них не распределена.","ekubo_ethereum.json")
        add(2116,"rfq","","9-17,29-45,57-64,71-90,105-123","После 22.12.2025 учтены RFQ actions и 48-байтовые события зарегистрированных Settler. В смешанной сделке одна нога подтверждена двумя переводами одного маркетмейкера. API собственного предложения требует ключа.","llama_top_0x_rfq.json.gz")
    return entries
