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
// не меняется; нет прокси, delegatecall, обновлений, произвольных вызовов, приёма BNB, selfdestruct. Обратный вызов пула
// платит только пулу текущего шага (адрес записан в transient на время вызова), не больше суммы шага и один раз.
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

    // Пределы цены «без ограничения» — TickMath.MIN_SQRT_RATIO + 1 / MAX_SQRT_RATIO − 1 (Uniswap/v3-core@5ea68a77
    // contracts/libraries/TickMath.sol L14, L16; те же числа у Pancake V3, Slipstream, Algebra V1.9, ks-elastic-sc).
    uint160 internal constant MIN_PRICE = 4295128739 + 1;
    uint160 internal constant MAX_PRICE = 1461446703485210103287273052203988822378723970342 - 1;
    // Доли и комиссии: 10000 bps = весь баланс; комиссия V2 — в долях 1e-6 (как Pool.fee_pips бота).
    uint256 internal constant BPS = 10000;
    uint256 internal constant PIPS = 1000000;

    // Шаг маршрута. shareBps — доля ТЕКУЩЕГО баланса исполнителя по tokenIn (10000 — весь остаток).
    // data: V2 — abi.encode(uint32 feePips); CURVE — abi.encode(int128 i, int128 j); остальные — пусто.
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

    // Обратный вызов текущего шага: кто может позвать, каким токеном и сколько максимум платить (EIP-1153).
    address transient expPool;
    address transient expToken;
    uint256 transient expMax;
    bool transient locked;

    constructor() {
        owner = msg.sender;
    }

    // Исполняет маршрут. Вход amountIn токена tokenIn забирается у владельца (нужен approve на этот контракт);
    // выход tokenOut и все остатки — владельцу; получено владельцем < minOut — откат. Возвращает: получено.
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
        if (amountIn > 0) {
            _call(tokenIn, abi.encodeCall(IERC20.transferFrom, (msg.sender, address(this), amountIn)));
        }
        uint256 before = IERC20(tokenOut).balanceOf(msg.sender);
        for (uint256 i = 0; i < steps.length; ++i) {
            _step(steps[i], steps[i].tokenOut == tokenOut ? msg.sender : address(this));
        }
        _sweep(tokenIn);
        _sweep(tokenOut);
        for (uint256 i = 0; i < steps.length; ++i) {
            _sweep(steps[i].tokenIn);
            _sweep(steps[i].tokenOut);
        }
        received = IERC20(tokenOut).balanceOf(msg.sender) - before;
        require(received >= minOut, "minOut");
        locked = false;
    }

    // Один шаг: сумма = баланс исполнителя по tokenIn × shareBps / 10000; выход — `to` (владелец, если это токен
    // выхода маршрута; иначе сам исполнитель — для следующих шагов).
    function _step(Step calldata s, address to) internal {
        require(s.shareBps > 0 && s.shareBps <= BPS, "bps");
        require(s.tokenIn != s.tokenOut, "tokens");
        uint256 amount = IERC20(s.tokenIn).balanceOf(address(this)) * s.shareBps / BPS;
        require(amount > 0, "amount");
        bool zeroIn = s.tokenIn < s.tokenOut; // token0 < token1 — пулы сортируют токены по адресу (фабрики V2/V3)
        uint8 k = s.kind;
        if (k == V2 || k == SOLIDLY) {
            _transfer(s.tokenIn, s.pool, amount);
            uint256 out = k == V2 ? _v2Out(s, zeroIn) : _solidlyOut(s, zeroIn);
            // swap(uint256,uint256,address,bytes) — одна подпись у пары V2 и у пары Solidly (интерфейсы выше).
            IV2Pair(s.pool).swap(zeroIn ? 0 : out, zeroIn ? out : 0, to, "");
        } else if (k == V3 || k == ALGEBRA) {
            _expect(s.pool, s.tokenIn, amount);
            IV3Pool(s.pool).swap(to, zeroIn, _int(amount), zeroIn ? MIN_PRICE : MAX_PRICE, "");
        } else if (k == ELASTIC) {
            _expect(s.pool, s.tokenIn, amount);
            IElasticPool(s.pool).swap(to, _int(amount), zeroIn, zeroIn ? MIN_PRICE : MAX_PRICE, "");
        } else if (k == MAVERICK) {
            _expect(s.pool, s.tokenIn, amount);
            bool aIn = IMaverickV2Pool(s.pool).tokenA() == s.tokenIn;
            IMaverickV2Pool.SwapParams memory p =
                IMaverickV2Pool.SwapParams(amount, aIn, false, aIn ? type(int32).max : type(int32).min);
            IMaverickV2Pool(s.pool).swap(to, p, abi.encode(s.tokenIn)); // непустые data — иначе пул не зовёт оплату
        } else if (k == CURVE) {
            (int128 i, int128 j) = abi.decode(s.data, (int128, int128));
            _call(s.tokenIn, abi.encodeCall(IERC20.approve, (s.pool, amount))); // ровно на шаг
            ICurvePool(s.pool).exchange(i, j, amount, 0); // выход — исполнителю (msg.sender пула), затем _sweep
        } else if (k == DODO) {
            _transfer(s.tokenIn, s.pool, amount);
            if (IDodoPool(s.pool)._BASE_TOKEN_() == s.tokenIn) IDodoPool(s.pool).sellBase(to);
            else IDodoPool(s.pool).sellQuote(to);
        } else if (k == WOO) {
            uint256 had = IERC20(s.tokenIn).balanceOf(s.pool);
            _transfer(s.tokenIn, s.pool, amount);
            uint256 got = IERC20(s.tokenIn).balanceOf(s.pool) - had; // дошло до пула (налог токена)
            IWooPP(s.pool).swap(s.tokenIn, s.tokenOut, got, 0, to, address(this));
        } else {
            revert("kind");
        }
        expPool = address(0);
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

    // Выход Solidly: вход = баланс пары − резерв, выход — getAmountOut самой пары (её комиссия и кривая stable/volatile).
    function _solidlyOut(Step calldata s, bool zeroIn) internal view returns (uint256) {
        (uint256 r0, uint256 r1,) = ISolidlyPair(s.pool).getReserves();
        uint256 got = IERC20(s.tokenIn).balanceOf(s.pool) - (zeroIn ? r0 : r1);
        return ISolidlyPair(s.pool).getAmountOut(got, s.tokenIn);
    }

    // Обратные вызовы пулов: V3 Uniswap/Slipstream (IUniswapV3SwapCallback.sol L16, ICLSwapCallback.sol L16),
    // PancakeSwap V3 (IPancakeV3SwapCallback.sol L16), Algebra (IAlgebraSwapCallback.sol L18), KyberSwap Elastic
    // (ISwapCallback.sol L16), Maverick V2 (IMaverickV2SwapCallback.sol L10). Положительная дельта — долг пулу.
    function uniswapV3SwapCallback(int256 d0, int256 d1, bytes calldata) external { _pay(d0 > 0 ? d0 : d1); }
    function pancakeV3SwapCallback(int256 d0, int256 d1, bytes calldata) external { _pay(d0 > 0 ? d0 : d1); }
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
