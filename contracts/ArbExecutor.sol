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
// STABLE_U256_BNB (обёртка WBNB при withdraw или пул шага при exchange) и на время withdraw в executeNative;
// после транзакции BNB на контракте нет.
// Версии: v1 — только execute (артефакты contracts/out/v1/, развёрнутые до решения 272); v2 — + executeNative
// (нативный BNB монетой сделки, решение 272). Селектор и ABI execute у v1 и v2 одинаковы. У v2 конструктор принимает
// адрес обёртки нативной монеты (wnative, слот 1 — один раз, сеттера нет; ревью 272, А1).
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
    // (bnbIn: true — монета i пула нативная, tokenIn = WBNB; false — монета j нативная, tokenOut = WBNB); остальные — пусто.
    struct Step {
        uint8 kind;
        address pool;
        address tokenIn;
        address tokenOut;
        uint16 shareBps;
        bytes data;
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
    // Первый шаг вида «вход переводом в пул» (V2, SOLIDLY, DODO, WOO, NOMI_STABLE, KYBER_DMM), берущий ВЕСЬ вход (10000 bps, tokenIn маршрута),
    // получает вход transferFrom(владелец → пул) напрямую, без захода в исполнитель: налог перевода — один раз
    // (как у Universal Router через Permit2). Для цикла (tokenIn = tokenOut) — как обычно (итог меряется после забора);
    // цикл — ровно одна доля: вход маршрута берёт только первый шаг и целиком (10000 bps). Иначе выход вида с выходом
    // на исполнитель (CURVE / STABLE_U256 / SADDLE / STABLE_U256_BNB) — тот же токен — посчитался бы входом следующей
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
    //   оставшийся на исполнителе (виды CURVE / STABLE_U256 / SADDLE). Легальные маршруты (доли, многошаговые, шаг
    //   STABLE_U256_BNB) так не ходят: вход маршрута только расходуется, выход только копится.
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
        } else if (k == DODO) {
            _send(s.tokenIn, s.pool, amount, fromOwner > 0);
            if (IDodoPool(s.pool)._BASE_TOKEN_() == s.tokenIn) IDodoPool(s.pool).sellBase(to);
            else IDodoPool(s.pool).sellQuote(to);
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
    function algebraSwapCallback(int256 d0, int256 d1, bytes calldata) external { _pay(d0 > 0 ? d0 : d1); }
    function swapCallback(int256 d0, int256 d1, bytes calldata) external { _pay(d0 > 0 ? d0 : d1); }

    function maverickV2SwapCallback(address tokenIn, uint256 amountIn, uint256, bytes calldata) external {
        require(tokenIn == expToken, "token");
        _pay(_int(amountIn));
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
        return k == V2 || k == SOLIDLY || k == DODO || k == WOO || k == NOMI_STABLE || k == KYBER_DMM;
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
