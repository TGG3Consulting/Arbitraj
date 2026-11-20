// SPDX-License-Identifier: MIT
// Файл: contracts/ArbExecutor.sol
// Что делает: минимальный исполнитель маршрута DEX бота ARB (решение 122, часть X; проект
//   docs/notes/smart-router-design-2026-11-03.md). Одна транзакция: забрать вход у владельца → шаги по пулам разных DEX
//   → весь выход и все остатки владельцу → проверка «владелец получил ≥ minOut», иначе откат всей транзакции (решение 40).
//   Денег не хранит: в конце каждой транзакции баланс исполнителя по токенам маршрута = 0.
// Кто вызывает: только кошелёк бота (владелец) — exec/executor/* (src/venues/bsc_dex).
// От чего зависит: contracts/IPools.sol — официальные интерфейсы пулов (ссылки у каждого интерфейса).
// Настройки: адрес развёрнутого контракта — exec.executor_address [bsc_dex].
//
// Безопасность (разбор — contracts/README.md): владелец задаётся один раз в конструкторе (развёртывающий кошелёк) и
// не меняется; нет прокси, delegatecall, обновлений, произвольных вызовов, selfdestruct. Обратный вызов пула
// платит только пулу текущего шага (адрес записан в transient на время вызова), не больше суммы шага и один раз.
// BNB (receive) принимается только от одного адреса, записанного в transient на время одного вызова шага
// STABLE_U256_BNB (обёртка WBNB при withdraw или пул шага при exchange), на время withdraw в executeNative и шага
// EKUBO с нативным ETH (обёртка при withdraw, ядро Ekubo при выплате — решение 454);
// после транзакции BNB на контракте нет.
// Версии: v1 — только execute (артефакты contracts/out/v1/, развёрнутые до решения 272); v2 — + executeNative
// (нативный BNB монетой сделки, решение 272). Селектор и ABI execute у v1 и v2 одинаковы. У v2 конструктор принимает
// адрес обёртки нативной монеты (wnative, слот 1 — один раз, сеттера нет; ревью 272, А1). v3 (решение 395, t181) —
// + шаги пулов внутри общего менеджера: UNI_V4 (Uniswap V4 PoolManager: unlock → unlockCallback), INF_CL и INF_BIN
// (PancakeSwap Infinity Vault: lock → lockAcquired; Infinity Stable — пул в крючке пула CL, тот же INF_CL). execute,
// executeNative, хранилище (owner — слот 0, wnative — слот 1) и прежние виды — без изменений.
// v4 (решения 440, 441, 445, 447, 454, 456) — + шаги FLUID = 22 (решение 440: пул Fluid DEX T1, нативная монета пула
// через обёртку), BALANCER_V3 = 23 (решение 441, T433-F2: пул Balancer V3 — Vault V3 unlock → balancerV3Callback →
// swap / settle / sendTo), LB20 = 24 (решение 445: пара Liquidity Book v2.0, LFJ V2 Arbitrum), EKUBO = 25 (решение
// 447: пул Ekubo в ядре Core — lock → locked_6416899205; решение 454 — нативный ETH ключа и forward только у
// MEVCapture); всё прочее — как v3. Номера шагов v4 — решение 456 (слияние семейств Arbitrum, без пересечений).
// Артефакты contracts/out/ — ещё v3: сборка solc (contracts/build.sh) — вне агентов; пересобирать — только как v4, не на
// месте v3 (бот: Version::V4, src/venues/bsc_dex/pools/exec_callback.rs since).
// v5 (решение 472, T472-08; Version::V5 заведён и в ветке t240, решение 470) — + шаг CURVE_ETH = 28 (пул Curve с
// нативным ETH: exchange{value}(int128, int128, uint256, uint256), обёртка withdraw / deposit); всё прочее — как v4.
// Сборки v5 нет: contracts/out/ — v4; пересобрать как v5 — вне агентов (docs/TASKS.md «Решение 472», «нужна сборка v5»).
pragma solidity 0.8.37;

import "./IPools.sol";

contract ArbExecutor {
    // Виды шагов (кодировщики бота — src/venues/bsc_dex/exec/executor/kind.rs, те же числа).
    uint8 internal constant V2 = 0;
    uint8 internal constant V3 = 1;
    uint8 internal constant ALGEBRA = 2;
    uint8 internal constant SOLIDLY = 3;
    uint8 internal constant CURVE = 4;
    uint8 internal constant DODO = 5;
    uint8 internal constant WOO = 6;
    uint8 internal constant MAVERICK = 7;
    uint8 internal constant ELASTIC = 8;
    uint8 internal constant STABLE_U256 = 9; // PancakeSwap StableSwap, Lista: exchange(uint256 i, uint256 j, dx, 0)
    uint8 internal constant SADDLE = 10; // Nerve (Saddle): swap(uint8 i, uint8 j, dx, 0, block.timestamp)
    // PancakeSwap StableSwap / Lista, у пула одна из монет пары — нативный BNB (coins(k) = BNB_ADDRESS): вход —
    // WBNB.withdraw → exchange{value}; выход — BNB от пула → WBNB.deposit. Токены шага — WBNB (ERC-20), не 0xEeee….
    uint8 internal constant STABLE_U256_BNB = 11;
    // Решение 284 (только v2): пара Nomiswap Stable — вход переводом в пару, выход = getAmountOut(tokenIn, вход) самой
    // пары (порядок аргументов обратный SOLIDLY), swap как у V2.
    uint8 internal constant NOMI_STABLE = 12;
    // Решение 285 (только v2): пул Metric OMM — swap(to, zeroForOne, +сумма, предел 0 / uint128.max, «01», «») +
    // metricOmmSwapCallback; data = abi.encode(bool zeroForOne) — порядок токенов пула не по адресу (бот знает его по
    // getImmutables пула). Неверный флаг — пул спросит другой токен, _pay платит только tokenIn шага → откат.
    uint8 internal constant METRIC = 13;
    // Решение 286 (только v2): пул iZiSwap / Biswap V3 — swapX2Y(to, сумма, int24.min, «») при tokenIn < tokenOut (tokenX —
    // меньший адрес, фабрики сортируют пару), иначе swapY2X(to, сумма, int24.max, «»); пределы точек пул сам обрезает до
    // leftMostPt / rightMostPt. Оплата — swapX2YCallback (x) / swapY2XCallback (y) → _pay. data пусто.
    uint8 internal constant IZI = 14;
    // Решение 287 (только v2): пул KyberSwap Classic DMM — вход переводом в пул, выход по getTradeInfo() пула (виртуальные
    // резервы и комиссия этого блока, формула DMMLibrary.getAmountOut), swap как у V2 (та же подпись).
    uint8 internal constant KYBER_DMM = 15;
    // Решение 288 (только v2): пара SmarDex — swap(to, zeroForOne по порядку адресов, +сумма, «») + smardexSwapCallback →
    // _pay (пара платит выход до обратного вызова, затем проверяет, что вход пришёл). data пусто.
    uint8 internal constant SMARDEX = 16;
    // Решение 289 (только v2): пул Balancer V2 (ACSI Finance) — обмен в Vault: s.pool = Vault, data = abi.encode(bytes32
    // poolId); approve Vault ровно на шаг → Vault.swap(GIVEN_IN, вход, получатель `to`) → разрешение после шага — 0.
    uint8 internal constant BALANCER = 17;
    // Решение 290 (только v2): пул Liquidity Book V2.1 (LFJ) — вход переводом в пул, swap(swapForY, to): пул сам считает
    // вход = баланс − резерв и платит выход `to` по бинам (без обратного вызова); swapForY = (getTokenX() пула == tokenIn)
    // — порядок X/Y задаёт создатель пула, не адреса. data пусто.
    uint8 internal constant LB = 18;
    // Решение 395 (t181, только v3): пулы внутри общего менеджера. s.pool — контракт, который открывают для обмена:
    // UNI_V4 — PoolManager Uniswap V4 (unlock / unlockCallback), INF_CL / INF_BIN — Vault PancakeSwap Infinity (lock /
    // lockAcquired; обмен — у key.poolManager: CLPoolManager / BinPoolManager). data = abi.encode(ключ пула): UNI_V4 —
    // V4PoolKey, INF_* — InfPoolKey (IPools.sol). Направление — по токенам шага (tokenIn == currency0 → zeroForOne /
    // swapForY); токены шага должны быть ровно валютами ключа; нативная валюта (address 0) — не шаг исполнителя.
    uint8 internal constant UNI_V4 = 19;
    uint8 internal constant INF_CL = 20;
    uint8 internal constant INF_BIN = 21;
    // Решение 440 (только v4): пул Fluid DEX T1 — approve пулу ровно на шаг + swapIn(swap0to1, сумма, 0, to); вход пул
    // списывает transferFrom(исполнитель → слой ликвидности) в своём liquidityCallback; data = abi.encode(bool swap0to1,
    // uint8 native, address liquidity) — native 1: вход в нативной монете пула (токен шага — обёртка: withdraw →
    // swapIn{value}), 2: выход в нативной монете (приходит от liquidity → deposit в обёртку), 0: оба токена ERC-20.
    uint8 internal constant FLUID = 22;
    // Решение 441 (только v4): пул Balancer V3. s.pool — хранилище Vault V3, data = abi.encode(address пула). Шаг открывает
    // хранилище (unlock), обмен и расчёт — в balancerV3Callback (только от s.pool и только с данными шага — хеш в
    // transient, как у UNI_V4): swap(EXACT_IN, сумма, предел 1) → перевод ровно суммы хранилищу + settle → sendTo выхода
    // получателю шага. Недоплата (налог перевода): settle засчитывает min(подсказка, прирост резерва хранилища)
    // (Vault.sol L138-L156) — кредит меньше долга, дельта не нулевая, unlock откатится всё (BalanceNotSettled).
    uint8 internal constant BALANCER_V3 = 23;
    // Решение 445: пара Liquidity Book v2.0 (LFJ V2, Arbitrum) — вход переводом в пару, swap(swapForY, to): пара сама
    // считает вход = баланс − резерв − накопленные комиссии и платит выход `to` по бинам (без обратного вызова);
    // swapForY = (tokenX() пары == tokenIn) — у v2.0 геттер tokenX(), getTokenX (шаг LB) у неё нет. data пусто.
    uint8 internal constant LB20 = 24;
    // Решение 447: пул Ekubo в ядре Core (EkuboProtocol/evm-contracts@61d09daa). s.pool — ядро; data = abi.encode(bytes32
    // config, bool native, bool forward) ключа пула (token0 / token1 — токены шага по порядку адресов; решение 454:
    // native — token0 ключа нативный ETH, его сторона шага — обёртка wnative). Core.lock → locked_6416899205 → обмен:
    // forward = false — swap_6269342730 ядра, forward = true (MEVCapture) — Core.forward(расширение) (иначе расширение
    // откатит обмен: MEVCapture.sol L84-L86); долг входа — startPayments / перевод / completePayments (нативный — ETH
    // ядру value), выход — withdraw (нативный — ETH исполнителю → deposit в обёртку).
    uint8 internal constant EKUBO = 25;
    // Решение 472 (T472-08, только v5): пул Curve с нативным ETH — монета 0xEeee… пула (curvefi/curve-factory f389123
    // Plain2ETH.vy / Plain3ETH.vy / Plain4ETH.vy, 5272fd2 Plain2ETHEMA.vy: coins[0] = 0xEeee…, Plain2ETH L133).
    // exchange(int128, int128, uint256, uint256) @payable (L537-L546: перегрузка с _receiver = msg.sender): вход ETH —
    // `assert msg.value == _dx` (L580), выход ETH — raw_call(_receiver, b"", value=dy) (L606, весь газ). data =
    // abi.encode(int128 i, int128 j, bool ethIn); токены шага — обёртка wnative и встречная монета (_curveEth).
    uint8 internal constant CURVE_ETH = 28;
    // Пределы цены Ekubo MIN_SQRT_RATIO_RAW / MAX_SQRT_RATIO_RAW (evm-contracts@61d09daa src/types/sqrtRatio.sol L13, L15):
    // тот же предел ставит Router (withDefaultSqrtRatioLimit, swapParameters.sol L74-L82).
    uint256 internal constant EKUBO_MIN_SQRT = 4611797791050542631;
    uint256 internal constant EKUBO_MAX_SQRT = 79227682466138141934206691491;
    // BNB_ADDRESS пулов StableSwap (PancakeStableSwapTwoPool.sol L39, Lista StableSwapPool.sol L54) — метка нативной
    // монеты, не ERC-20: токеном шага быть не может (у шагов с BNB токен шага — обёртка WBNB).
    address internal constant NATIVE = 0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE;

    // Пределы цены «без ограничения» — TickMath.MIN_SQRT_RATIO + 1 / MAX_SQRT_RATIO − 1 (Uniswap/v3-core@5ea68a77
    // contracts/libraries/TickMath.sol L14, L16; те же числа у Pancake V3, Slipstream, Algebra V1.9, ks-elastic-sc).
    uint160 internal constant MIN_PRICE = 4295128739 + 1;
    uint160 internal constant MAX_PRICE = 1461446703485210103287273052203988822378723970342 - 1;
    // Доли и комиссии: 10000 bps = весь баланс; комиссия V2 — в долях 1e-6 (как Pool.fee_pips бота).
    uint256 internal constant BPS = 10000;
    uint256 internal constant PIPS = 1000000;
    // Точность комиссии пула KyberSwap DMM (dmm-smart-contracts@e557b57 MathExt.sol L9: PRECISION = 10**18).
    uint256 internal constant DMM_PRECISION = 1e18;

    // Шаг маршрута. shareBps — доля ТЕКУЩЕГО баланса исполнителя по tokenIn (10000 — весь остаток).
    // data: V2 — abi.encode(uint32 feePips); CURVE — abi.encode(int128 i, int128 j); STABLE_U256 — abi.encode(uint256 i,
    // uint256 j); SADDLE — abi.encode(uint8 i, uint8 j); STABLE_U256_BNB — abi.encode(uint256 i, uint256 j, bool bnbIn)
    // (bnbIn: true — монета i пула нативная, tokenIn = WBNB; false — монета j нативная, tokenOut = WBNB); METRIC — abi.encode(bool
    // zeroForOne); BALANCER — abi.encode(bytes32 poolId); BALANCER_V3 — abi.encode(address pool); UNI_V4 —
    // abi.encode(V4PoolKey); INF_CL / INF_BIN — abi.encode(InfPoolKey); FLUID — abi.encode(bool swap0to1, uint8 native,
    // address liquidity); EKUBO — abi.encode(bytes32 config, bool native, bool forward) (решение 454); CURVE_ETH —
    // abi.encode(int128 i, int128 j, bool ethIn) (решение 472: ethIn — монета i нативная, tokenIn = обёртка; иначе
    // монета j нативная, tokenOut = обёртка); остальные — пусто.
    struct Step {
        uint8 kind;
        address pool;
        address tokenIn;
        address tokenOut;
        uint16 shareBps;
        bytes data;
    }

    // Решение 454: данные шага EKUBO для обратного вызова ядра (abi.encode — слова подряд, как кортеж полей).
    struct EkuboCb {
        address tIn;
        address tOut;
        uint256 amount;
        address to;
        bytes32 config;
        bool native;
        bool fwd;
    }

    // Кошелёк бота. Слот хранилища 0 (contracts/out/ArbExecutor.layout.json) — его подставляет сухая сделка.
    address public owner;
    // Ревью 272 (А1): обёртка нативной монеты (WBNB; бот передаёт dex.native_wrapped при развёртывании) — задаётся ОДИН
    // раз в конструкторе, сеттера нет; executeNative берёт только её. Хранилище (слот 1), а не immutable: immutable
    // вшивается в код в сети, и код разных развёртываний перестал бы быть одинаковым (побайтная сверка onchain.rs).
    // Бот читает слот при сверке (onchain.rs) и сравнивает с dex.native_wrapped; сухая сделка подставляет его.
    address public wnative;

    // Обратный вызов текущего шага: кто может позвать, каким токеном и сколько максимум платить (EIP-1153).
    address transient expPool;
    address transient expToken;
    uint256 transient expMax;
    bool transient locked;
    // От кого receive() примет BNB сейчас: обёртка WBNB на время withdraw или пул шага STABLE_U256_BNB на время
    // exchange; в остальное время 0 — любой приход BNB откатывается. Отдельно от expPool: пул с выходом в BNB не
    // должен получить право на _pay (expToken/expMax прошлого шага).
    address transient bnbFrom;
    // Решение 395 (v3): хеш данных обратного вызова менеджера текущего шага (UNI_V4 / INF_*). unlockCallback /
    // lockAcquired исполняет обмен только от expPool (менеджер / Vault шага) и только с этими данными — один раз.
    bytes32 transient expHash;

    // Решение 272: итог нативной стороны executeNative — пришло BNB (value) и ушло владельцу BNB (остаток / покупка).
    event NativeSettled(uint256 nativeIn, uint256 nativeOut);

    // Принимает: адрес обёртки нативной монеты (с кодом — иначе «wnative»). Владелец — развёртывающий кошелёк.
    constructor(address wnative_) {
        require(wnative_.code.length > 0, "wnative");
        owner = msg.sender;
        wnative = wnative_;
    }

    // Исполняет маршрут. Вход amountIn токена tokenIn забирается у владельца (нужен approve на этот контракт);
    // выход tokenOut и все остатки — владельцу; получено владельцем < minOut — откат. Возвращает: получено.
    // Первый шаг вида «вход переводом в пул» (V2, SOLIDLY, DODO, WOO, NOMI_STABLE, KYBER_DMM, LB, LB20), берущий ВЕСЬ вход (10000 bps, tokenIn маршрута),
    // получает вход transferFrom(владелец → пул) напрямую, без захода в исполнитель: налог перевода — один раз
    // (как у Universal Router через Permit2). Для цикла (tokenIn = tokenOut) — как обычно (итог меряется после забора);
    // цикл — ровно одна доля: вход маршрута берёт только первый шаг и целиком (10000 bps). Иначе выход вида с выходом
    // на исполнитель (CURVE / STABLE_U256 / SADDLE / STABLE_U256_BNB / CURVE_ETH) — тот же токен — посчитался бы входом следующей
    // доли (ревью).
    function execute(
        address tokenIn,
        uint256 amountIn,
        address tokenOut,
        uint256 minOut,
        uint256 deadline,
        Step[] calldata steps
    ) external returns (uint256 received) {
        require(msg.sender == owner, "owner");
        require(!locked, "lock");
        locked = true;
        require(block.timestamp <= deadline, "deadline");
        bool direct = amountIn > 0 && tokenIn != tokenOut && steps.length > 0 && steps[0].tokenIn == tokenIn
            && steps[0].shareBps == BPS && _pushed(steps[0].kind);
        if (amountIn > 0 && !direct) {
            _call(tokenIn, abi.encodeCall(IERC20.transferFrom, (msg.sender, address(this), amountIn)));
        }
        uint256 before = IERC20(tokenOut).balanceOf(msg.sender);
        _steps(tokenIn, tokenOut, steps, direct ? amountIn : 0, msg.sender);
        _sweepAll(tokenIn, tokenOut, steps);
        _sweepBnb();
        received = IERC20(tokenOut).balanceOf(msg.sender) - before;
        require(received >= minOut, "minOut");
        locked = false;
    }

    // Решение 272 (исполнитель v2): маршрут с нативным BNB на краю — монета сделки BNB. Только владелец; вход/выход
    // маршрута по-прежнему ERC-20 обёртка wnative (из конструктора, ревью А1 — параметра нет), а с кошельком — нативный
    // BNB:
    // wrapIn = true (продажа BNB): value транзакции = amountIn, tokenIn = wnative → wnative.deposit{value} до шагов →
    //   шаги → выход tokenOut (ERC-20) владельцу, прирост его баланса ≥ minOut; несъеденный остаток wnative
    //   разворачивается и возвращается владельцу нативным BNB.
    // wrapIn = false (покупка BNB): value 0, tokenOut = wnative; вход ERC-20 — как у execute (прямой вход в пул первого
    //   шага разрешён); выход шагов в wnative — исполнителю → withdraw всего → нативный BNB владельцу; получено = BNB,
    //   реально пришедший от обёртки при withdraw (разность баланса исполнителя) ≥ minOut.
    // Ревью А2 («native step»): ни один шаг не берёт на вход токен выхода маршрута и не отдаёт токен входа маршрута.
    //   Покупка: выход шагов в обёртке копится на исполнителе (outTo = исполнитель) — шаг с tokenIn = обёртка потратил бы
    //   уже полученное ([USDT→WBNB, WBNB→USDT] проходил). Продажа: шаг с tokenOut = обёртка вернул бы обёртку в остаток
    //   входа (доли следующих шагов и возврат остатка считались бы с ним), шаг с tokenIn = выход — тратил бы выход,
    //   оставшийся на исполнителе (виды CURVE / STABLE_U256 / SADDLE). Легальные маршруты (доли, многошаговые, шаги
    //   STABLE_U256_BNB и CURVE_ETH — решение 472) так не ходят: вход маршрута только расходуется, выход только копится.
    // Ревью А3: событие NativeSettled(nativeIn, nativeOut) — только свои операции: продажа — (value, BNB, пришедший при
    //   развороте остатка обёртки), покупка — (0, BNB, пришедший при развороте выхода). Посторонний BNB (selfdestruct) и
    //   посторонний WBNB на исполнителе до шагов уходят владельцу (_sweepBnb, _sweep), но в событие и в «получено» не
    //   попадают. Факт бота — по событию: продажа — продано nativeIn − nativeOut, покупка — куплено nativeOut (по событиям
    //   WBNB считать нельзя: шаг STABLE_U256_BNB сам делает withdraw/deposit исполнителем).
    // Цикл (tokenIn = tokenOut) здесь не исполняется. Возвращает: получено владельцем (ERC-20 или wei BNB).
    function executeNative(
        bool wrapIn,
        address tokenIn,
        uint256 amountIn,
        address tokenOut,
        uint256 minOut,
        uint256 deadline,
        Step[] calldata steps
    ) external payable returns (uint256 received) {
        require(msg.sender == owner, "owner");
        require(!locked, "lock");
        locked = true;
        require(block.timestamp <= deadline, "deadline");
        require(tokenIn != tokenOut && amountIn > 0 && steps.length > 0, "native route");
        for (uint256 i = 0; i < steps.length; ++i) {
            require(steps[i].tokenIn != tokenOut && steps[i].tokenOut != tokenIn, "native step");
        }
        address w = wnative;
        _sweep(w); // посторонний WBNB до шагов — владельцу: в шаги, разворот и событие он не попадает
        if (wrapIn) {
            require(tokenIn == w && msg.value == amountIn, "wrap");
            received = _nativeIn(w, amountIn, tokenOut, steps);
        } else {
            require(tokenOut == w && msg.value == 0, "unwrap");
            received = _nativeOut(w, tokenIn, amountIn, steps);
        }
        require(received >= minOut, "minOut");
        locked = false;
    }

    // Продажа BNB: обернуть value, шаги, остаток обёртки — нативным BNB владельцу. Возвращает: прирост tokenOut владельца.
    function _nativeIn(address w, uint256 amountIn, address tokenOut, Step[] calldata steps)
        internal
        returns (uint256 received)
    {
        IWBNB(w).deposit{value: amountIn}();
        uint256 before = IERC20(tokenOut).balanceOf(msg.sender);
        _steps(w, tokenOut, steps, 0, msg.sender);
        uint256 back = _unwrapAll(w);
        _sweepAll(w, tokenOut, steps);
        _sweepBnb();
        received = IERC20(tokenOut).balanceOf(msg.sender) - before;
        emit NativeSettled(amountIn, back);
    }

    // Покупка BNB: вход ERC-20 (прямой вход в пул — как у execute), выход в обёртке — исполнителю, развернуть всё,
    // нативный BNB владельцу. Возвращает: BNB, пришедший от обёртки при развороте выхода шагов (wei; владелец получает
    // его в _sweepBnb — вместе с посторонним BNB, если он был, но «получено» и событие — без постороннего).
    function _nativeOut(address w, address tokenIn, uint256 amountIn, Step[] calldata steps)
        internal
        returns (uint256 received)
    {
        bool direct = steps[0].tokenIn == tokenIn && steps[0].shareBps == BPS && _pushed(steps[0].kind);
        if (!direct) {
            _call(tokenIn, abi.encodeCall(IERC20.transferFrom, (msg.sender, address(this), amountIn)));
        }
        _steps(tokenIn, w, steps, direct ? amountIn : 0, address(this));
        received = _unwrapAll(w);
        _sweepAll(tokenIn, w, steps);
        _sweepBnb();
        emit NativeSettled(0, received);
    }

    // Шаги маршрута по порядку. Выход шага, чей tokenOut — выход маршрута, идёт outTo (execute — владельцу; покупка
    // BNB — исполнителю, дальше withdraw); иначе исполнителю. Цикл (tokenIn = tokenOut) — ровно одна доля («loop»).
    // fromOwner — вход первого шага прямо с владельца в пул (0 — с баланса исполнителя).
    function _steps(address tokenIn, address tokenOut, Step[] calldata steps, uint256 fromOwner, address outTo)
        internal
    {
        bool loop = tokenIn == tokenOut;
        for (uint256 i = 0; i < steps.length; ++i) {
            if (loop) {
                require(i == 0 ? steps[0].tokenIn == tokenIn && steps[0].shareBps == BPS : steps[i].tokenIn != tokenIn, "loop");
            }
            address to = steps[i].tokenOut == tokenOut ? outTo : address(this);
            _step(steps[i], to, i == 0 ? fromOwner : 0);
        }
    }

    // Все токены маршрута с баланса исполнителя — владельцу.
    function _sweepAll(address tokenIn, address tokenOut, Step[] calldata steps) internal {
        _sweep(tokenIn);
        _sweep(tokenOut);
        for (uint256 i = 0; i < steps.length; ++i) {
            _sweep(steps[i].tokenIn);
            _sweep(steps[i].tokenOut);
        }
    }

    // Развернуть весь WBNB исполнителя: BNB придёт от wnative (receive пускает только bnbFrom) на время withdraw.
    // Возвращает: BNB, реально пришедший (разность баланса исполнителя до/после withdraw — посторонний BNB на контракте
    // сюда не входит; обёртка, не отдавшая BNB при withdraw, даёт 0).
    function _unwrapAll(address w) internal returns (uint256 got) {
        uint256 amount = IERC20(w).balanceOf(address(this));
        if (amount == 0) return 0;
        uint256 had = address(this).balance;
        bnbFrom = w;
        IWBNB(w).withdraw(amount);
        bnbFrom = address(0);
        got = address(this).balance - had;
    }

    // Один шаг: сумма = баланс исполнителя по tokenIn × shareBps / 10000 (или fromOwner — вход прямо с владельца в
    // пул, только у видов с переводом в пул); выход — `to` (владелец, если это токен выхода маршрута; иначе сам
    // исполнитель — для следующих шагов).
    function _step(Step calldata s, address to, uint256 fromOwner) internal {
        require(s.shareBps > 0 && s.shareBps <= BPS, "bps");
        require(s.tokenIn != s.tokenOut, "tokens");
        uint256 amount =
            fromOwner > 0 ? fromOwner : IERC20(s.tokenIn).balanceOf(address(this)) * s.shareBps / BPS;
        require(amount > 0, "amount");
        bool zeroIn = s.tokenIn < s.tokenOut; // token0 < token1 — пулы сортируют токены по адресу (фабрики V2/V3)
        uint8 k = s.kind;
        if (k == V2 || k == SOLIDLY || k == NOMI_STABLE || k == KYBER_DMM) {
            _send(s.tokenIn, s.pool, amount, fromOwner > 0);
            uint256 out = k == V2 ? _v2Out(s, zeroIn) : k == KYBER_DMM ? _dmmOut(s, zeroIn) : _pairOut(s, zeroIn, k == NOMI_STABLE);
            // swap(uint256,uint256,address,bytes) — одна подпись у пары V2, Solidly, Nomiswap Stable и KyberSwap DMM (IPools.sol).
            IV2Pair(s.pool).swap(zeroIn ? 0 : out, zeroIn ? out : 0, to, "");
        } else if (k == V3 || k == ALGEBRA) {
            _expect(s.pool, s.tokenIn, amount);
            IV3Pool(s.pool).swap(to, zeroIn, _int(amount), zeroIn ? MIN_PRICE : MAX_PRICE, "");
        } else if (k == METRIC) {
            _expect(s.pool, s.tokenIn, amount);
            bool zf = abi.decode(s.data, (bool));
            require(amount <= uint256(uint128(type(int128).max)), "int");
            // Непустые callbackData — как у пробного вызова на цепи (пустые не проверялись).
            IMetricOmmPool(s.pool).swap(to, zf, int128(int256(amount)), zf ? 0 : type(uint128).max, hex"01", "");
        } else if (k == IZI) {
            _expect(s.pool, s.tokenIn, amount);
            require(amount <= type(uint128).max, "int");
            if (zeroIn) IIziPool(s.pool).swapX2Y(to, uint128(amount), type(int24).min, "");
            else IIziPool(s.pool).swapY2X(to, uint128(amount), type(int24).max, "");
        } else if (k == SMARDEX) {
            _expect(s.pool, s.tokenIn, amount);
            ISmardexPair(s.pool).swap(to, zeroIn, _int(amount), "");
        } else if (k == BALANCER) {
            _balancer(s, amount, to);
        } else if (k == ELASTIC) {
            _expect(s.pool, s.tokenIn, amount);
            IElasticPool(s.pool).swap(to, _int(amount), zeroIn, zeroIn ? MIN_PRICE : MAX_PRICE, "");
        } else if (k == MAVERICK) {
            _expect(s.pool, s.tokenIn, amount);
            bool aIn = IMaverickV2Pool(s.pool).tokenA() == s.tokenIn;
            IMaverickV2Pool.SwapParams memory p =
                IMaverickV2Pool.SwapParams(amount, aIn, false, aIn ? type(int32).max : type(int32).min);
            IMaverickV2Pool(s.pool).swap(to, p, abi.encode(s.tokenIn)); // непустые data — иначе пул не зовёт оплату
        } else if (k == CURVE || k == STABLE_U256 || k == SADDLE) {
            _stable(s, amount);
        } else if (k == STABLE_U256_BNB) {
            _stableBnb(s, amount);
        } else if (k == CURVE_ETH) {
            _curveEth(s, amount);
        } else if (k == DODO) {
            _send(s.tokenIn, s.pool, amount, fromOwner > 0);
            if (IDodoPool(s.pool)._BASE_TOKEN_() == s.tokenIn) IDodoPool(s.pool).sellBase(to);
            else IDodoPool(s.pool).sellQuote(to);
        } else if (k == LB) {
            // LBPair.swap (joe-v2@v2.1.0 src/LBPair.sol L483-L557): вход = баланс пула − резерв (L487, BinHelper.sol
            // L290-L306), нулевой вход — откат L488, бинов не хватило — откат OutOfLiquidity (L538), нулевой выход — откат
            // L544; выход — transfer `to` (L552-L556). getTokenX — аргумент клона (L132-L134).
            _send(s.tokenIn, s.pool, amount, fromOwner > 0);
            ILBPair(s.pool).swap(ILBPair(s.pool).getTokenX() == s.tokenIn, to);
        } else if (k == LB20) {
            // LBPair v2.0 swap (lfj-gg/joe-v2@v2.0.0 src/LBPair.sol L335-L429): вход = баланс − резерв − комиссии пары
            // (L343-L345, TokenHelper.sol L61-L71), нулевой вход — откат L347, бинов не хватило — откат дерева бинов
            // (TreeMath.sol L60); выход — safeTransfer `to` (L422-L428). tokenX — переменная пары (L56, initialize L131).
            _send(s.tokenIn, s.pool, amount, fromOwner > 0);
            ILBPairV20(s.pool).swap(ILBPairV20(s.pool).tokenX() == s.tokenIn, to);
        } else if (k == UNI_V4 || k == INF_CL || k == INF_BIN) {
            _singleton(s, amount, to);
        } else if (k == FLUID) {
            _fluid(s, amount, to);
        } else if (k == BALANCER_V3) {
            _balancerV3(s, amount, to);
        } else if (k == EKUBO) {
            _ekubo(s, amount, to);
        } else if (k == WOO) {
            uint256 had = IERC20(s.tokenIn).balanceOf(s.pool);
            _send(s.tokenIn, s.pool, amount, fromOwner > 0);
            uint256 got = IERC20(s.tokenIn).balanceOf(s.pool) - had; // дошло до пула (налог токена)
            IWooPP(s.pool).swap(s.tokenIn, s.tokenOut, got, 0, to, address(this));
        } else {
            revert("kind");
        }
        expPool = address(0);
    }

    // StableSwap — Curve / Ellipsis (int128), с индексами uint256 (Pancake, Lista) и Saddle (Nerve): approve пулу ровно на шаг, пул сам списывает
    // transferFrom(исполнитель, пул, dx); выход — исполнителю (msg.sender пула), затем _sweep; выход должен вырасти.
    // Срок Saddle — block.timestamp (срок всей транзакции уже проверен в execute). Разрешение после шага — 0.
    function _stable(Step calldata s, uint256 amount) internal {
        require(s.tokenIn != NATIVE && s.tokenOut != NATIVE, "native");
        _call(s.tokenIn, abi.encodeCall(IERC20.approve, (s.pool, amount)));
        uint256 had = IERC20(s.tokenOut).balanceOf(address(this));
        if (s.kind == CURVE) {
            (int128 i, int128 j) = abi.decode(s.data, (int128, int128));
            ICurvePool(s.pool).exchange(i, j, amount, 0);
        } else if (s.kind == STABLE_U256) {
            (uint256 i, uint256 j) = abi.decode(s.data, (uint256, uint256));
            IStableSwapU256(s.pool).exchange(i, j, amount, 0);
        } else {
            (uint8 i, uint8 j) = abi.decode(s.data, (uint8, uint8));
            ISaddleSwap(s.pool).swap(i, j, amount, 0, block.timestamp);
        }
        require(IERC20(s.tokenOut).balanceOf(address(this)) > had, "out");
        if (IERC20(s.tokenIn).allowance(address(this), s.pool) > 0) {
            _call(s.tokenIn, abi.encodeCall(IERC20.approve, (s.pool, 0)));
        }
    }

    // Balancer V2 (решение 289): s.pool — Vault, data — poolId. Vault сам списывает вход transferFrom(исполнитель → Vault,
    // сумма шага) (Swaps.sol L100, AssetTransfersHandler.sol L76) — approve ровно на шаг; выход Vault переводит `to`
    // (Swaps.sol L101). GIVEN_IN = 0 (IVault.sol L498), limit 1 — нулевой выход откатывает (Swaps.sol L98), итог маршрута
    // проверяет minOut; срок — block.timestamp (срок транзакции проверен в execute). Разрешение после шага — 0.
    function _balancer(Step calldata s, uint256 amount, address to) internal {
        bytes32 poolId = abi.decode(s.data, (bytes32));
        _call(s.tokenIn, abi.encodeCall(IERC20.approve, (s.pool, amount)));
        IBalancerVault(s.pool).swap(
            IBalancerVault.SingleSwap(poolId, 0, s.tokenIn, s.tokenOut, amount, ""),
            IBalancerVault.FundManagement(address(this), false, payable(to), false),
            1,
            block.timestamp
        );
        if (IERC20(s.tokenIn).allowance(address(this), s.pool) > 0) {
            _call(s.tokenIn, abi.encodeCall(IERC20.approve, (s.pool, 0)));
        }
    }

    // StableSwap Pancake / Lista с нативным BNB (STABLE_U256_BNB). Пул: монета BNB_ADDRESS входит как msg.value
    // (`require(dx == msg.value)`), выходит `msg.sender.call{gas: bnb_gas, value: dy}("")` (PancakeStableSwapTwoPool.sol
    // L457-L464, L644-L663; StableSwapPool.sol L507-L514, L697-L716). Токены шага — WBNB (ERC-20) и встречная монета.
    // bnbIn: WBNB.withdraw(сумма) (BNB приходит от WBNB — bnbFrom = WBNB) → exchange{value: сумма}(i, j, сумма, 0) без
    //   approve; встречная монета — исполнителю, должна вырасти.
    // !bnbIn: approve пулу ровно на шаг → exchange(i, j, сумма, 0) (BNB приходит от пула — bnbFrom = пул) → пришедший
    //   от пула BNB (разность баланса) → WBNB.deposit; WBNB исполнителя должен вырасти; разрешение после шага — 0.
    // Сумма value = ровно сумма шага (снята с WBNB тем же шагом) — чужой BNB на контракте в обмен не уходит.
    function _stableBnb(Step calldata s, uint256 amount) internal {
        require(s.tokenIn != NATIVE && s.tokenOut != NATIVE, "native");
        (uint256 i, uint256 j, bool bnbIn) = abi.decode(s.data, (uint256, uint256, bool));
        uint256 had = IERC20(s.tokenOut).balanceOf(address(this));
        if (bnbIn) {
            require(s.tokenIn.code.length > 0, "token code");
            bnbFrom = s.tokenIn;
            IWBNB(s.tokenIn).withdraw(amount);
            bnbFrom = address(0);
            IStableSwapU256(s.pool).exchange{value: amount}(i, j, amount, 0);
        } else {
            _call(s.tokenIn, abi.encodeCall(IERC20.approve, (s.pool, amount)));
            uint256 bnbHad = address(this).balance;
            bnbFrom = s.pool;
            IStableSwapU256(s.pool).exchange(i, j, amount, 0);
            bnbFrom = address(0);
            if (IERC20(s.tokenIn).allowance(address(this), s.pool) > 0) {
                _call(s.tokenIn, abi.encodeCall(IERC20.approve, (s.pool, 0)));
            }
            require(s.tokenOut.code.length > 0, "token code");
            // Ревью А3: в обёртку — только BNB, пришедший от пула этим шагом (посторонний BNB остаётся и уходит
            // владельцу в _sweepBnb, в выход шага не попадает).
            IWBNB(s.tokenOut).deposit{value: address(this).balance - bnbHad}();
        }
        require(IERC20(s.tokenOut).balanceOf(address(this)) > had, "out");
    }

    // Решение 472 (T472-08, v5): пул Curve с нативным ETH (CURVE_ETH, ссылки — у константы). Как _stableBnb, но
    // exchange(int128, int128, uint256, uint256) Curve (ICurveEthPool). Токены шага — обёртка (ERC-20) и встречная монета.
    // ethIn: обёртка.withdraw(сумма) (ETH приходит от обёртки — bnbFrom = обёртка) → exchange{value: сумма}(i, j, сумма, 0)
    //   без approve; встречная монета — исполнителю, должна вырасти.
    // !ethIn: approve пулу ровно на шаг → exchange(i, j, сумма, 0) (ETH приходит от пула — bnbFrom = пул) → пришедший от
    //   пула ETH (разность баланса) → обёртка.deposit; обёртка исполнителя должна вырасти; разрешение после шага — 0.
    function _curveEth(Step calldata s, uint256 amount) internal {
        require(s.tokenIn != NATIVE && s.tokenOut != NATIVE, "native");
        (int128 i, int128 j, bool ethIn) = abi.decode(s.data, (int128, int128, bool));
        uint256 had = IERC20(s.tokenOut).balanceOf(address(this));
        if (ethIn) {
            require(s.tokenIn.code.length > 0, "token code");
            bnbFrom = s.tokenIn;
            IWBNB(s.tokenIn).withdraw(amount);
            bnbFrom = address(0);
            ICurveEthPool(s.pool).exchange{value: amount}(i, j, amount, 0);
        } else {
            _call(s.tokenIn, abi.encodeCall(IERC20.approve, (s.pool, amount)));
            uint256 ethHad = address(this).balance;
            bnbFrom = s.pool;
            ICurveEthPool(s.pool).exchange(i, j, amount, 0);
            bnbFrom = address(0);
            if (IERC20(s.tokenIn).allowance(address(this), s.pool) > 0) {
                _call(s.tokenIn, abi.encodeCall(IERC20.approve, (s.pool, 0)));
            }
            require(s.tokenOut.code.length > 0, "token code");
            // Только ETH, пришедший от пула этим шагом (посторонний остаётся и уходит владельцу в _sweepBnb).
            IWBNB(s.tokenOut).deposit{value: address(this).balance - ethHad}();
        }
        require(IERC20(s.tokenOut).balanceOf(address(this)) > had, "out");
    }

    // Решение 440 (v4): пул Fluid DEX T1 (Instadapp/fluid-contracts-public@404d73f poolT1/coreModule/core/main.sol
    // L51-L338 _swapIn, L689-L700 swapIn, L889-L905 liquidityCallback). Вход ERC-20 списывает сам пул:
    // safeTransferFrom(исполнитель → слой ликвидности) в liquidityCallback — approve пулу ровно на шаг, разрешение после
    // шага — 0. Вход в нативной монете пула (native = 1): msg.value == сумма (L83-L86) — WBNB-обёртка шага withdraw
    // (нативная монета приходит от обёртки — bnbFrom) → swapIn{value}. Выход в нативной монете (native = 2): слой
    // ликвидности шлёт её call с газом 50000 (libraries/safeTransfer.sol L71-L83) — receive пускает только liquidity из
    // data (bnbFrom) на время swapIn; пришедшее этим шагом → deposit в обёртку; выход шага — исполнителю (дальше
    // _sweep), должен вырасти. Порог выхода шага 0 — итог маршрута проверяет minOut.
    function _fluid(Step calldata s, uint256 amount, address to) internal {
        (bool zf, uint8 nat, address liq) = abi.decode(s.data, (bool, uint8, address));
        if (nat == 1) {
            require(s.tokenIn.code.length > 0, "token code");
            bnbFrom = s.tokenIn;
            IWBNB(s.tokenIn).withdraw(amount);
            bnbFrom = address(0);
            IFluidDexT1(s.pool).swapIn{value: amount}(zf, amount, 0, to);
            return;
        }
        require(nat <= 2, "native");
        _call(s.tokenIn, abi.encodeCall(IERC20.approve, (s.pool, amount)));
        if (nat == 2) {
            require(s.tokenOut.code.length > 0, "token code");
            uint256 had = IERC20(s.tokenOut).balanceOf(address(this));
            uint256 bnbHad = address(this).balance;
            bnbFrom = liq;
            IFluidDexT1(s.pool).swapIn(zf, amount, 0, address(this));
            bnbFrom = address(0);
            IWBNB(s.tokenOut).deposit{value: address(this).balance - bnbHad}();
            require(IERC20(s.tokenOut).balanceOf(address(this)) > had, "out");
        } else {
            IFluidDexT1(s.pool).swapIn(zf, amount, 0, to);
        }
        if (IERC20(s.tokenIn).allowance(address(this), s.pool) > 0) {
            _call(s.tokenIn, abi.encodeCall(IERC20.approve, (s.pool, 0)));
        }
    }

    // Приём BNB — только от bnbFrom (transient). Укладывается в 2300 газа (стипендия transfer у WBNB.withdraw; пулы
    // шлют bnb_gas ≥ 2300 + стипендия 2300): только TLOAD (100 газа, EIP-1153) и сравнение, без SSTORE / событий.
    receive() external payable {
        require(msg.sender == bnbFrom, "bnb");
    }

    // Выход V2: вход = баланс пары − резерв (налог токена учтён), формула UniswapV2Library.getAmountOut с комиссией
    // feePips/1e6 (Pancake 2500, Uniswap 3000 — числа бота из dex.v2_fee_pct / комиссии пары).
    function _v2Out(Step calldata s, bool zeroIn) internal view returns (uint256) {
        uint256 fee = abi.decode(s.data, (uint32));
        require(fee < PIPS, "fee");
        (uint256 r0, uint256 r1,) = IV2Pair(s.pool).getReserves();
        (uint256 rIn, uint256 rOut) = zeroIn ? (r0, r1) : (r1, r0);
        uint256 inFee = (IERC20(s.tokenIn).balanceOf(s.pool) - rIn) * (PIPS - fee);
        return inFee * rOut / (rIn * PIPS + inFee);
    }

    // Выход KyberSwap DMM (решение 287): вход = баланс пула − реальный резерв (как в swap пула, DMMPool.sol L188-L193),
    // формула DMMLibrary.getAmountOut (DMMLibrary.sol L75-L91): вход·(1e18 − комиссия)/1e18, затем x·y=k по виртуальным
    // резервам. Комиссия — getTradeInfo этого же блока (та, которую пул применит в проверке K). Выход ≥ реального резерва
    // пул откатит («DMM: INSUFFICIENT_LIQUIDITY»).
    function _dmmOut(Step calldata s, bool zeroIn) internal view returns (uint256) {
        (uint256 r0, uint256 r1, uint256 v0, uint256 v1, uint256 fee) = IDmmPool(s.pool).getTradeInfo();
        uint256 got = IERC20(s.tokenIn).balanceOf(s.pool) - (zeroIn ? r0 : r1);
        return zeroIn ? _dmmFormula(got, v0, v1, fee) : _dmmFormula(got, v1, v0, fee);
    }

    // Формула DMMLibrary.getAmountOut (DMMLibrary.sol L75-L91) по уже полученному входу: вход·(1e18 − комиссия)/1e18,
    // затем x·y=k по виртуальным резервам. Отдельная функция — из-за «Stack too deep» в _dmmOut.
    function _dmmFormula(uint256 got, uint256 vIn, uint256 vOut, uint256 fee) internal pure returns (uint256) {
        uint256 inFee = got * (DMM_PRECISION - fee) / DMM_PRECISION;
        return inFee * vOut / (vIn + inFee);
    }

    // Выход Solidly / Nomiswap Stable: вход = баланс пары − резерв, выход — getAmountOut самой пары (её комиссия и
    // кривая). Резервы — первые два слова getReserves у обеих (uint256 Solidly, uint112 Nomiswap — ABI-слово то же).
    // Nomiswap: getAmountOut(tokenIn, amountIn) (NomiswapStablePair.sol L251); Solidly: getAmountOut(amountIn, tokenIn).
    function _pairOut(Step calldata s, bool zeroIn, bool nomi) internal view returns (uint256) {
        (uint256 r0, uint256 r1,) = ISolidlyPair(s.pool).getReserves();
        uint256 got = IERC20(s.tokenIn).balanceOf(s.pool) - (zeroIn ? r0 : r1);
        if (nomi) return INomiStablePair(s.pool).getAmountOut(s.tokenIn, got);
        return ISolidlyPair(s.pool).getAmountOut(got, s.tokenIn);
    }

    // Обратные вызовы пулов: V3 Uniswap/Slipstream (IUniswapV3SwapCallback.sol L16, ICLSwapCallback.sol L16),
    // PancakeSwap V3 (IPancakeV3SwapCallback.sol L16), Algebra (IAlgebraSwapCallback.sol L18), KyberSwap Elastic
    // (ISwapCallback.sol L16), Maverick V2 (IMaverickV2SwapCallback.sol L10), SquadSwap V3 (решение 281, задача t114:
    // SquadV3Pool — форк PancakeSwap V3 с тем же swap, зовёт ISquadV3SwapCallback.squadV3SwapCallback(int256,int256,bytes),
    // селектор 0xc4e21d86; verified-исходник Sourcify partial_match 56/0x9fb9815ca1417b06466aaa86521a28ba83d0bd33,
    // SquadV3Pool.sol L794, L800 — тот же путь и та же проверка вызывающего _pay, что у V3). Положительная дельта — долг пулу.
    function uniswapV3SwapCallback(int256 d0, int256 d1, bytes calldata) external { _pay(d0 > 0 ? d0 : d1); }
    function pancakeV3SwapCallback(int256 d0, int256 d1, bytes calldata) external { _pay(d0 > 0 ? d0 : d1); }
    function squadV3SwapCallback(int256 d0, int256 d1, bytes calldata) external { _pay(d0 > 0 ? d0 : d1); }
    // Решение 285: Metric OMM — тот же _pay (только пул текущего шага, долг ≤ суммы шага, токен входа шага).
    function metricOmmSwapCallback(int256 d0, int256 d1, bytes calldata) external { _pay(d0 > 0 ? d0 : d1); }
    // Решение 286: iZiSwap / Biswap V3 — x / y: сколько исполнитель платит (IiZiSwapCallback.sol L19-L39); тот же _pay.
    function swapX2YCallback(uint256 x, uint256, bytes calldata) external { _pay(_int(x)); }
    function swapY2XCallback(uint256, uint256 y, bytes calldata) external { _pay(_int(y)); }
    // Решение 288: SmarDex — дельты пары (ISmardexSwapCallback.sol L21), положительная — долг; тот же _pay.
    function smardexSwapCallback(int256 d0, int256 d1, bytes calldata) external { _pay(d0 > 0 ? d0 : d1); }
    function algebraSwapCallback(int256 d0, int256 d1, bytes calldata) external { _pay(d0 > 0 ? d0 : d1); }
    function swapCallback(int256 d0, int256 d1, bytes calldata) external { _pay(d0 > 0 ? d0 : d1); }

    function maverickV2SwapCallback(address tokenIn, uint256 amountIn, uint256, bytes calldata) external {
        require(tokenIn == expToken, "token");
        _pay(_int(amountIn));
    }

    // Решение 395 (v3): шаг пула внутри общего менеджера (UNI_V4 / INF_CL / INF_BIN). Открывает менеджер (s.pool):
    // Uniswap V4 — PoolManager.unlock (v4-core PoolManager.sol L104-L114), Infinity — Vault.lock (infinity-core Vault.sol
    // L74-L86); менеджер вызывает у исполнителя unlockCallback / lockAcquired с теми же данными — там обмен и расчёт
    // (_singletonCb). Обратный вызов пускается только от s.pool и только с данными этого шага (хеш в transient) — после
    // него expPool и expHash стёрты; не было вызова — откат «cb». expToken / expMax прошлых шагов стираются до открытия. Сумма ≤ int128 (дельты менеджеров — int128).
    function _singleton(Step calldata s, uint256 amount, address to) internal {
        require(amount <= uint256(uint128(type(int128).max)), "int");
        bytes memory cb = abi.encode(s.kind, s.tokenIn, s.tokenOut, amount, to, s.data);
        // Ревью t181 (Н5): метки оплаты _pay прошлых шагов (expToken / expMax) стираются — менеджер этого шага
        // (expPool) не может получить через V3-обратный вызов токен и сумму прошлого шага (_pay откатит «owed»).
        (expPool, expToken, expMax) = (s.pool, address(0), 0);
        expHash = keccak256(cb);
        if (s.kind == UNI_V4) IV4PoolManager(s.pool).unlock(cb);
        else IInfVault(s.pool).lock(cb);
        require(expHash == bytes32(0), "cb");
    }

    // Обратные вызовы менеджеров (v4-core IUnlockCallback.sol L9; infinity-core ILockCallback.sol L9). Ответ пуст.
    function unlockCallback(bytes calldata data) external returns (bytes memory) {
        _singletonCb(data, true);
        return "";
    }

    function lockAcquired(bytes calldata data) external returns (bytes memory) {
        _singletonCb(data, false);
        return "";
    }

    // Обмен внутри открытого менеджера: только менеджер текущего шага (expPool) и только данные шага (expHash), один раз
    // (оба стираются до обмена — повторный вход из крючка откатится «pool»). Обмен с точным входом amount (amountSpecified
    // = −amount; предел цены CL — MIN+1 / MAX−1, у Bin предела нет). Затем — дельты ИСПОЛНИТЕЛЯ у менеджера (как
    // DeltaResolver периферии: _getFullDebt / _getFullCredit): по токену входа долг 0 < долг ≤ amount (крючок не может
    // взять больше суммы шага), по токену выхода — кредит > 0; долг гасится sync → перевод ровно долга менеджеру →
    // settle (оплачено = прирост баланса менеджера — токен с налогом на перевод недоплатит, менеджер откатит), кредит
    // забирается take прямо получателю шага `to` (владелец — если это выход маршрута). После этого дельт у исполнителя
    // нет, иначе менеджер откатит всё (CurrencyNotSettled). Ошибки: «pool», «kind», «key», «native», «owed», «out».
    function _singletonCb(bytes calldata data, bool uni) internal {
        address m = expPool;
        require(m != address(0) && msg.sender == m && keccak256(data) == expHash, "pool");
        expPool = address(0);
        expHash = bytes32(0);
        (uint8 k, address tIn, address tOut, uint256 amount, address to, bytes memory kd) =
            abi.decode(data, (uint8, address, address, uint256, address, bytes));
        require(uni == (k == UNI_V4), "kind");
        if (uni) _swapV4(m, kd, tIn, tOut, amount);
        else _swapInf(k, kd, tIn, tOut, amount);
        int256 debt = _delta(m, tIn, uni);
        int256 credit = _delta(m, tOut, uni);
        require(debt < 0 && uint256(-debt) <= amount, "owed");
        require(credit > 0, "out");
        ISingletonSettle(m).sync(tIn);
        _transfer(tIn, m, uint256(-debt));
        ISingletonSettle(m).settle();
        ISingletonSettle(m).take(tOut, to, uint256(credit));
    }

    // Uniswap V4: PoolManager.swap(ключ, (zeroForOne, −amount, предел), «») (v4-core PoolManager.sol L187-L227).
    function _swapV4(address m, bytes memory kd, address tIn, address tOut, uint256 amount) internal {
        V4PoolKey memory key = abi.decode(kd, (V4PoolKey));
        bool zf = _dir(key.currency0, key.currency1, tIn, tOut);
        IV4PoolManager(m).swap(key, V4SwapParams(zf, -int256(amount), zf ? MIN_PRICE : MAX_PRICE), "");
    }

    // Infinity: обмен у менеджера пула из ключа (CLPoolManager.sol L162-L212 / BinPoolManager.sol L133-L177); дельта
    // пишется в Vault на вызывающего — исполнитель (VaultAppDeltaSettlement.sol L12-L21). Менеджер чужого Vault или
    // незарегистрированный в нём — Vault откатит (onlyRegisteredApp / NoLocker).
    function _swapInf(uint8 k, bytes memory kd, address tIn, address tOut, uint256 amount) internal {
        InfPoolKey memory key = abi.decode(kd, (InfPoolKey));
        bool zf = _dir(key.currency0, key.currency1, tIn, tOut);
        if (k == INF_CL) {
            InfClSwapParams memory p = InfClSwapParams(zf, -int256(amount), zf ? MIN_PRICE : MAX_PRICE);
            IInfClPoolManager(key.poolManager).swap(key, p, "");
        } else {
            require(k == INF_BIN, "kind");
            IInfBinPoolManager(key.poolManager).swap(key, zf, -int128(int256(amount)), "");
        }
    }

    // Решение 441: шаг Balancer V3 (v4). Хранилище Vault V3 (s.pool) по unlock зовёт у вызывающего ровно переданные данные
    // (Vault.sol 20241204-v3-vault L133-L135) — это вызов balancerV3Callback(cb); после unlock обратный вызов обязан
    // быть («cb»). Метки оплаты прошлых шагов стираются (как в _singleton), expMax = 0 — _pay из хранилища откатится.
    function _balancerV3(Step calldata s, uint256 amount, address to) internal {
        address pool = abi.decode(s.data, (address));
        bytes memory cb = abi.encode(pool, s.tokenIn, s.tokenOut, amount, to);
        (expPool, expToken, expMax) = (s.pool, address(0), 0);
        expHash = keccak256(cb);
        IBalancerV3Vault(s.pool).unlock(abi.encodeWithSelector(this.balancerV3Callback.selector, cb));
        require(expHash == bytes32(0), "cb");
    }

    // Обмен внутри открытого хранилища V3: только хранилище текущего шага (expPool) и только данные шага (expHash), один
    // раз (оба стираются до обмена). swap(EXACT_IN = 0, пул, вход, выход, сумма, предел 1, «») (Vault.sol L197-L291;
    // нулевой выход — откат SwapLimit) → хранилище должно было взять ровно сумму («owed») и дать выход > 0 («out») →
    // перевод ровно суммы хранилищу, settle(вход, сумма) (Vault.sol L138-L156: кредит = прирост баланса, не больше
    // подсказки) → sendTo(выход, получатель шага, выход) (L158-L163). Дельты исполнителя после этого нулевые — иначе
    // хранилище откатит unlock. Ответ пуст.
    function balancerV3Callback(bytes calldata cb) external returns (bytes memory) {
        address v = expPool;
        require(v != address(0) && msg.sender == v && keccak256(cb) == expHash, "pool");
        expPool = address(0);
        expHash = bytes32(0);
        // kind = 0 (EXACT_IN) и userData пусто — значения по умолчанию; поля — прямо в параметры (меньше переменных стека).
        IBalancerV3Vault.VaultSwapParams memory p;
        address to;
        (p.pool, p.tokenIn, p.tokenOut, p.amountGivenRaw, to) =
            abi.decode(cb, (address, address, address, uint256, address));
        p.limitRaw = 1;
        (, uint256 inRaw, uint256 outRaw) = IBalancerV3Vault(v).swap(p);
        require(inRaw == p.amountGivenRaw, "owed");
        require(outRaw > 0, "out");
        _transfer(p.tokenIn, v, inRaw);
        IBalancerV3Vault(v).settle(p.tokenIn, inRaw);
        IBalancerV3Vault(v).sendTo(p.tokenOut, to, outRaw);
        return "";
    }

    // Решение 447: шаг пула Ekubo. Открывает ядро (s.pool): Core.lock() с данными шага после селектора
    // (FlashAccountant.sol L146-L187) — ядро вызывает locked_6416899205 у исполнителя с теми же данными; пускается только
    // от s.pool и только с данными этого шага (хеш в transient), один раз; не было вызова — откат «cb». Метки оплаты
    // V3-обратных вызовов прошлых шагов стираются. Сумма ≤ int128 (SwapParameters.amount — int128).
    // Решение 454: data = abi.encode(bytes32 config, bool native, bool forward). native — token0 ключа нативный ETH
    // (адрес 0); его сторона шага — обёртка wnative (из конструктора; иначе «wnative»). forward — обмен через
    // Core.forward(расширение) (MEVCapture); иначе обычный swap ядра (без расширения и TWAMM / Oracle / BoostedFees —
    // их beforeSwap зовёт само ядро; forward к ним исполнил бы чужую команду расширения).
    function _ekubo(Step calldata s, uint256 amount, address to) internal {
        require(amount <= uint256(uint128(type(int128).max)), "int");
        (bytes32 config, bool native, bool fwd) = abi.decode(s.data, (bytes32, bool, bool));
        require(!native || s.tokenIn == wnative || s.tokenOut == wnative, "wnative");
        bytes memory cb = abi.encode(EkuboCb(s.tokenIn, s.tokenOut, amount, to, config, native, fwd));
        (expPool, expToken, expMax) = (s.pool, address(0), 0);
        expHash = keccak256(cb);
        _ekuboCall(s.pool, abi.encodePacked(IEkuboCore.lock.selector, cb));
        require(expHash == bytes32(0), "cb");
    }

    // Обратный вызов ядра Ekubo (IFlashAccountant.sol L7: селектор 0x00000000; данные шага — после id, msg.data[36:]).
    // Только ядро текущего шага и только данные шага, один раз. Токены ключа — токены шага, обёртка нативной стороны
    // (native) — адрес 0. Обмен — _ekuboSwap, оплата долга — _ekuboPay, выход — _ekuboTake. Ошибки: «pool», «native».
    function locked_6416899205(uint256) external {
        address core = expPool;
        bytes calldata data = msg.data[36:];
        require(core != address(0) && msg.sender == core && keccak256(data) == expHash, "pool");
        expPool = address(0);
        expHash = bytes32(0);
        EkuboCb memory c = abi.decode(data, (EkuboCb));
        require(c.tIn != address(0) && c.tOut != address(0), "native");
        address w = c.native ? wnative : address(0);
        address kIn = c.tIn == w ? address(0) : c.tIn;
        address kOut = c.tOut == w ? address(0) : c.tOut;
        (uint256 owe, uint256 got) = _ekuboSwap(core, kIn, kOut, c);
        _ekuboPay(core, kIn, c.tIn, owe);
        _ekuboTake(core, kOut, c.tOut, c.to, got);
    }

    // Обмен на точный вход: SwapParameters = предел цены << 160 | сумма << 32 | isToken1 << 31 | skipAhead 0
    // (createSwapParameters, swapParameters.sol L42-L58); isToken1 — по адресам ключа (нативная — 0, всегда token0). forward = false —
    // swap_6269342730 ядра (ключ 3 слова + параметры, Core.sol L510-L525); true — forward(расширение) с abi.encode(ключ,
    // параметры) (MEVCapture.sol L177-L179, ответ — abi.encode(balanceUpdate, stateAfter), L258). balanceUpdate —
    // delta0 << 128 | delta1 (poolBalanceUpdate.sol L4-L25). Возвращает: (долг входа, выход); 0 < долг ≤ суммы шага
    // (иначе «owed»), выход > 0 (иначе «out»).
    function _ekuboSwap(address core, address kIn, address kOut, EkuboCb memory c)
        internal
        returns (uint256 owe, uint256 got)
    {
        bool isToken1 = kIn > kOut;
        (address t0, address t1) = isToken1 ? (kOut, kIn) : (kIn, kOut);
        uint256 limit = isToken1 ? EKUBO_MAX_SQRT : EKUBO_MIN_SQRT;
        bytes32 params = bytes32((limit << 160) | (c.amount << 32) | (isToken1 ? uint256(1) << 31 : 0));
        address ext = address(uint160(uint256(c.config) >> 96));
        bytes memory r = !c.fwd
            ? _ekuboCall(core, abi.encodePacked(bytes4(0), abi.encode(t0, t1, c.config), params))
            : _ekuboCall(core, abi.encodePacked(IEkuboCore.forward.selector, abi.encode(ext, t0, t1, c.config, params)));
        (bytes32 balanceUpdate,) = abi.decode(r, (bytes32, bytes32));
        int128 d0 = int128(uint128(uint256(balanceUpdate) >> 128));
        int128 d1 = int128(uint128(uint256(balanceUpdate)));
        (int128 o, int128 g) = isToken1 ? (d1, d0) : (d0, d1);
        require(o > 0 && uint256(uint128(o)) <= c.amount, "owed");
        require(g < 0, "out");
        (owe, got) = (uint256(uint128(o)), uint256(uint128(-g)));
    }

    // Оплата долга входа. ERC-20 — startPayments / перевод ровно долга / completePayments (оплачено = прирост баланса
    // ядра: токен с налогом недоплатит — ядро откатит DebtsNotZeroed). Решение 454, нативный ETH (kIn = 0): обёртка
    // разворачивается ровно на долг (ETH приходит от неё — bnbFrom) и ETH уходит ядру value без данных — receive ядра
    // гасит долг нативной монеты текущего locker (FlashAccountant.sol L384-L393). Ошибки: «eth pay».
    function _ekuboPay(address core, address kIn, address tIn, uint256 owe) internal {
        if (kIn != address(0)) {
            _ekuboCall(core, abi.encodePacked(IEkuboCore.startPayments.selector, abi.encode(tIn)));
            _transfer(tIn, core, owe);
            _ekuboCall(core, abi.encodePacked(IEkuboCore.completePayments.selector, abi.encode(tIn)));
            return;
        }
        require(tIn.code.length > 0, "token code");
        bnbFrom = tIn;
        IWBNB(tIn).withdraw(owe);
        bnbFrom = address(0);
        (bool ok,) = core.call{value: owe}("");
        require(ok, "eth pay");
    }

    // Выход. ERC-20 — withdraw ядра прямо получателю шага `to` (FlashAccountant.sol L322-L381: token 20 байт | получатель
    // 20 | сумма 16). Решение 454, нативный ETH (kOut = 0): withdraw исполнителю (ядро шлёт ETH call{value} —
    // L348-L356; receive пускает только ядро — bnbFrom), пришедший ETH (разность баланса) — в обёртку deposit, обёртка —
    // `to` (если `to` не сам исполнитель).
    function _ekuboTake(address core, address kOut, address tOut, address to, uint256 got) internal {
        if (kOut != address(0)) {
            _ekuboCall(core, abi.encodePacked(IEkuboCore.withdraw.selector, tOut, to, uint128(got)));
            return;
        }
        require(tOut.code.length > 0, "token code");
        uint256 had = address(this).balance;
        bnbFrom = core;
        _ekuboCall(core, abi.encodePacked(IEkuboCore.withdraw.selector, address(0), address(this), uint128(got)));
        bnbFrom = address(0);
        uint256 eth = address(this).balance - had;
        IWBNB(tOut).deposit{value: eth}();
        if (to != address(this)) _transfer(tOut, to, eth);
    }

    // Вызов ядра Ekubo с готовыми данными (не ABI: ядро читает calldata само); откат ядра пробрасывается как есть.
    // Возвращает: ответ.
    function _ekuboCall(address core, bytes memory data) internal returns (bytes memory r) {
        bool ok;
        (ok, r) = core.call(data);
        if (!ok) {
            assembly {
                revert(add(r, 32), mload(r))
            }
        }
    }

    // Направление по валютам ключа: токены шага — ровно currency0 / currency1 (иначе «key»), ERC-20 (нативная валюта
    // address 0 — «native»). Возвращает: tIn == currency0 (zeroForOne / swapForY).
    function _dir(address c0, address c1, address tIn, address tOut) internal pure returns (bool) {
        require(c0 != address(0), "native");
        require((tIn == c0 && tOut == c1) || (tIn == c1 && tOut == c0), "key");
        return tIn == c0;
    }

    // Дельта исполнителя у менеджера по валюте: Uniswap V4 — exttload(keccak256(исполнитель, валюта))
    // (TransientStateLibrary.sol L35-L43), Infinity — Vault.currencyDelta(исполнитель, валюта) (IVault.sol L56).
    function _delta(address m, address currency, bool uni) internal view returns (int256) {
        if (!uni) return IInfVault(m).currencyDelta(address(this), currency);
        return int256(uint256(IV4PoolManager(m).exttload(keccak256(abi.encode(address(this), currency)))));
    }

    // Оплата пулу текущего шага: только он, только токеном шага, не больше суммы шага, один раз.
    function _pay(int256 owed) internal {
        address pool = expPool;
        require(pool != address(0) && msg.sender == pool, "pool");
        require(owed > 0 && uint256(owed) <= expMax, "owed");
        expPool = address(0);
        _transfer(expToken, pool, uint256(owed));
    }

    function _expect(address pool, address token, uint256 max) internal {
        (expPool, expToken, expMax) = (pool, token, max);
    }

    // Весь баланс исполнителя по токену — владельцу.
    function _sweep(address token) internal {
        uint256 b = IERC20(token).balanceOf(address(this));
        if (b > 0) _transfer(token, owner, b);
    }

    // BNB на контракте после маршрута — владельцу. Шаги STABLE_U256_BNB сами BNB не оставляют (вход — ровно value
    // шага, выход — пришедший от пула BNB в WBNB); остаток бывает только присланным в обход receive (selfdestruct /
    // награда блока) — откат вместо возврата сделал бы исполнитель неработающим навсегда (1 wei извне).
    function _sweepBnb() internal {
        uint256 b = address(this).balance;
        if (b > 0) {
            (bool ok,) = owner.call{value: b}("");
            require(ok, "bnb sweep");
        }
    }

    // Виды, у которых вход переводится в пул заранее (а не платится в обратном вызове).
    function _pushed(uint8 k) internal pure returns (bool) {
        return k == V2 || k == SOLIDLY || k == DODO || k == WOO || k == NOMI_STABLE || k == KYBER_DMM || k == LB
            || k == LB20;
    }

    // Вход в пул: с баланса исполнителя или прямо с владельца (первый шаг, весь вход маршрута).
    function _send(address token, address to, uint256 amount, bool fromOwner) internal {
        if (fromOwner) _call(token, abi.encodeCall(IERC20.transferFrom, (owner, to, amount)));
        else _transfer(token, to, amount);
    }

    function _transfer(address token, address to, uint256 amount) internal {
        _call(token, abi.encodeCall(IERC20.transfer, (to, amount)));
    }

    // Вызов токена ERC-20: контракт есть, вызов прошёл, ответ пуст или true (токены без bool, EIP-20).
    function _call(address token, bytes memory data) internal {
        require(token.code.length > 0, "token code");
        (bool ok, bytes memory ret) = token.call(data);
        require(ok && (ret.length == 0 || abi.decode(ret, (bool))), "token call");
    }

    function _int(uint256 x) internal pure returns (int256) {
        require(x <= uint256(type(int256).max), "int");
        return int256(x);
    }
}
