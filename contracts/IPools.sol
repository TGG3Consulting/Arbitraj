// SPDX-License-Identifier: MIT
// Файл: contracts/IPools.sol
// Что делает: интерфейсы токена и пулов DEX для исполнителя ArbExecutor.sol — только функции, которые он вызывает,
//   с подписями из официальных исходников (репозиторий@коммит, файл, строки — у каждого интерфейса).
// Кто вызывает: contracts/ArbExecutor.sol (import).
// От чего зависит: ничего.
// Настройки: нет.
pragma solidity 0.8.37;

// ERC-20 — https://eips.ethereum.org/EIPS/eip-20
interface IERC20 {
    function balanceOf(address who) external view returns (uint256);
    function transfer(address to, uint256 value) external returns (bool);
    function transferFrom(address from, address to, uint256 value) external returns (bool);
    function approve(address spender, uint256 value) external returns (bool);
    function allowance(address owner, address spender) external view returns (uint256);
}

// Uniswap V2 / PancakeSwap V2 и форки: Uniswap/v2-core@6a9e7c97 contracts/UniswapV2Pair.sol L38, L159;
// pancakeswap/pancake-swap-core@3b214306 contracts/PancakePair.sol L38, L159.
interface IV2Pair {
    function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
    function swap(uint256 amount0Out, uint256 amount1Out, address to, bytes calldata data) external;
}

// Solidly (THENA V1, Velodrome V1): ThenafiBNB/THENA-Contracts@f5bc742e contracts/Pair.sol L239, L379, L459;
// velodrome-finance/v1 contracts/Pair.sol L198, L338, L416.
interface ISolidlyPair {
    function getReserves() external view returns (uint256 reserve0, uint256 reserve1, uint256 blockTimestampLast);
    function getAmountOut(uint256 amountIn, address tokenIn) external view returns (uint256);
    function swap(uint256 amount0Out, uint256 amount1Out, address to, bytes calldata data) external;
}

// Решение 470 (T470-06): пул Aero Basic — Dromos-Labs/metadex-public 79c8d5b6 V3/src/pools/Pool.sol L530 getReserves
// (три uint256), L548-L555 getAmountOutWithTotalFee (комиссия с MEV-налогом, msg.sender — вызывающий), L143 swap
// (подпись V2 — вызывается через IV2Pair). Интерфейс — V3/src/interfaces/pools/IPool.sol L269.
interface IAeroBasicPool {
    function getReserves() external view returns (uint256 reserve0, uint256 reserve1, uint256 blockTimestampLast);
    function getAmountOutWithTotalFee(uint256 amountIn, address tokenIn) external view returns (uint256);
}

// Nomiswap Stable (решение 284): пара NomiswapStablePair — verified-исходник Sourcify full match, chain 56, адрес
// 0x1e40450f8e21bb68490d7d91ab422888fb3d60f1 (те же исходники у 0x3a66…da87, 0xca25…a7ec), contracts/NomiswapStablePair.sol
// L132 swap (подпись как у пары V2), L251 getAmountOut(address tokenIn, uint256 amountIn) — порядок аргументов обратный
// Solidly (селектор 0xca706bcf), L276 getReserves (uint112). Вход — переводом в пару до swap (L146-L151).
interface INomiStablePair {
    function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
    function getAmountOut(address tokenIn, uint256 amountIn) external view returns (uint256);
    function swap(uint256 amount0Out, uint256 amount1Out, address to, bytes calldata data) external;
}

// Metric OMM (решение 285): исходник пулов BSC не опубликован; подпись swap — ABI официального SDK Metric (npm
// @nradko/metric-omm-sdk-v1@0.4.5 src/abis/MetricOmmPool.ts, github.com/Metric-OMM/metric-sdk), селектор 0xe24f0884 есть
// в коде пулов BSC (PUSH4; tests/data/metric_2026-11-15). amountSpecified > 0 — точный вход; пул зовёт у вызывающего
// metricOmmSwapCallback(int256,int256,bytes) (0xc3251075, PUSH32 в коде пула), положительная дельта — долг пулу (факт:
// eth_call с подменой кода — пробный контракт заплатил положительную дельту, обмен прошёл; пределы цены 0 / uint128.max).
interface IMetricOmmPool {
    function swap(address recipient, bool zeroForOne, int128 amountSpecified, uint128 priceLimitX64, bytes calldata callbackData, bytes calldata extensionData)
        external
        returns (int128, int128);
}

// SmarDex (решение 288): пара SmardexPair — фабрика BSC 0xA8EF6FEa013034E62E2C4A9Ec1CDb059fE23Af33, Sourcify match =
// SmarDex-Ecosystem/dex-smart-contracts@74d0444 deployments/bsc (solcInputs/1a3e5ed8…), contracts/core/SmardexPair.sol
// L155-L319 swap(to, zeroForOne, amountSpecified, data): amountSpecified > 0 — точный вход; выход переводится `to`, затем
// ISmardexSwapCallback(msg.sender).smardexSwapCallback(amount0, amount1, data) (ISmardexSwapCallback.sol L21) —
// положительная дельта — долг пары, проверка баланса входа после вызова (L268-L274, L280-L286). Вызывающий не ограничен.
interface ISmardexPair {
    function swap(address to, bool zeroForOne, int256 amountSpecified, bytes calldata data)
        external
        returns (int256 amount0, int256 amount1);
}

// KyberSwap Classic DMM (решение 287): KyberNetwork/dmm-smart-contracts@e557b57 contracts/DMMPool.sol L240-L265
// getTradeInfo (реальные и виртуальные резервы, комиссия блока над 1e18; при ampBps = BPS виртуальные = реальные), L162
// swap (подпись и селектор как у пары V2 — IV2Pair). Код BSC: фабрика 0x878dfe97…934c — Sourcify match (DMMPool в том же
// файле); вход — переводом в пул до swap (вход = баланс − резерв, L188-L193).
interface IDmmPool {
    function getTradeInfo()
        external
        view
        returns (uint112 reserve0, uint112 reserve1, uint112 vReserve0, uint112 vReserve1, uint256 feeInPrecision);
}

// Uniswap V3 / PancakeSwap V3 / Slipstream (Topaz) / Algebra V1.9 (THENA Fusion) — одна и та же подпись swap:
// Uniswap/v3-core@5ea68a77 contracts/interfaces/pool/IUniswapV3PoolActions.sol L75-L81;
// pancakeswap/pancake-v3-contracts@98684794 projects/v3-core/contracts/interfaces/pool/IPancakeV3PoolActions.sol L75-L81;
// velodrome-finance/slipstream@193a6b85 contracts/core/interfaces/pool/ICLPoolActions.sol L131-L137;
// npm @cryptoalgebra/v1.9-core@1.0.0 contracts/interfaces/pool/IAlgebraPoolActions.sol L96-L102.
interface IV3Pool {
    function swap(address recipient, bool zeroForOne, int256 amountSpecified, uint160 sqrtPriceLimitX96, bytes calldata data)
        external returns (int256 amount0, int256 amount1);
}

// KyberSwap Elastic: KyberNetwork/ks-elastic-sc@4ab08c0a contracts/interfaces/pool/IPoolActions.sol L88-L94.
interface IElasticPool {
    function swap(address recipient, int256 swapQty, bool isToken0, uint160 limitSqrtP, bytes calldata data)
        external returns (int256 qty0, int256 qty1);
}

// Curve / Ellipsis (плоский StableSwap, индексы int128): curvefi/curve-contract@574f4402
// contracts/pool-templates/base/SwapTemplateBase.vy L447; ellipsis-finance/ellipsis@6bdf7788 contracts/StableSwap.vy L436.
// Возвращаемое значение не читается (у Ellipsis его нет) — выход меряется по балансу.
interface ICurvePool {
    function exchange(int128 i, int128 j, uint256 dx, uint256 minDy) external;
}

// Решение 472 (T472-08): пул Curve с нативным ETH — curvefi/curve-factory f389123 contracts/implementations/plain-2/
// Plain2ETH.vy L537-L546 (`@payable`, exchange(i: int128, j: int128, _dx, _min_dy, _receiver = msg.sender) — перегрузка
// из 4 аргументов), та же подпись — Plain3ETH.vy / Plain4ETH.vy, 5272fd2 Plain2ETHEMA.vy. Возврат (uint256) не
// декодируется (как ICurvePool): выход шага меряется балансом.
interface ICurveEthPool {
    function exchange(int128 i, int128 j, uint256 dx, uint256 minDy) external payable;
}

// Решение 485: скважина Basin — BeanstalkFarms/Basin@ecf69233 src/Well.sol L207-L217 (swapFrom: IERC20 fromToken,
// IERC20 toToken, amountIn, minAmountOut, recipient, deadline → amountOut; IERC20 в ABI — address). Возврат не
// декодируется: выход уходит получателю шага.
interface IBasinWell {
    function swapFrom(
        address fromToken,
        address toToken,
        uint256 amountIn,
        uint256 minAmountOut,
        address recipient,
        uint256 deadline
    ) external returns (uint256 amountOut);
}

// PancakeSwap StableSwap (индексы uint256, payable, ничего не возвращает): pancakeswap/pancake-smart-contracts@cb079908
// projects/stable-swap/contracts/PancakeStableSwapTwoPool.sol L425-L430 (репозиторий закрыт — копия в форке
// migo557/pancake-smart-contracts, 1:1 с verified-кодом BscScan пула 0x6D8f…CE5A5 в tintinweb/smart-contract-sanctuary-bsc);
// Lista: lista-dao/moolah@eb7d73a src/dex/StableSwapPool.sol L478 (тот же заголовок + whenNotPaused и checkPriceDiff).
// Селектор exchange(uint256,uint256,uint256,uint256) = 0x5b41b908. Вход списывает пул transferFrom(msg.sender, пул, dx),
// выход — msg.sender (transfer_out). Монета BNB_ADDRESS (0xEeee…): вход — msg.value == dx (TwoPool L457-L460, Lista
// L507-L510), выход — msg.sender.call{gas: bnb_gas, value: dy}("") (TwoPool L644-L663, Lista L697-L716; bnb_gas 2300…23000,
// по умолчанию 4029 — TwoPool L25-L26, L35; Lista L36-L37, L151) — шаг STABLE_U256_BNB.
interface IStableSwapU256 {
    function exchange(uint256 i, uint256 j, uint256 dx, uint256 minDy) external payable;
}

// WBNB (обёртка BNB в BSC — копия WETH9): gnosis/canonical-weth@0dd1ea3e contracts/WETH9.sol L25-L42 — deposit()
// payable зачисляет msg.value; withdraw(wad) списывает и шлёт BNB `msg.sender.transfer(wad)` (стипендия 2300 газа);
// тот же интерфейс — lista-dao/moolah@eb7d73a src/provider/interfaces/IWBNB.sol L5-L7.
interface IWBNB {
    function deposit() external payable;
    function withdraw(uint256 wad) external;
}

// Saddle / Nerve: nerve-finance/contracts@bda7578 contracts/Swap.sol L389-L402 (swap(uint8 from, uint8 to, dx, minDy,
// deadline) → SwapUtils.swap: transferFrom(msg.sender, пул, dx), вход = сколько дошло (transferredDx), выход —
// msg.sender). Возврат не читается — выход меряется по балансу.
interface ISaddleSwap {
    function swap(uint8 tokenIndexFrom, uint8 tokenIndexTo, uint256 dx, uint256 minDy, uint256 deadline)
        external returns (uint256);
}

// DODO V2 (DVM/DPP/DSP): DODOEX/contractV2@2f1bcdac contracts/DODOVendingMachine/impl/DVMTrader.sol L41, L65,
// DVMStorage.sol L30 (_BASE_TOKEN_); DPPTrader.sol, DSPTrader.sol — те же sellBase/sellQuote.
interface IDodoPool {
    function _BASE_TOKEN_() external view returns (address);
    function sellBase(address to) external returns (uint256);
    function sellQuote(address to) external returns (uint256);
}

// WooFi WooPPV2: woonetwork/WooPoolV2@f73a0f64 contracts/WooPPV2.sol L152-L170 (вход заранее в пул, L443).
interface IWooPP {
    function swap(address fromToken, address toToken, uint256 fromAmount, uint256 minToAmount, address to, address rebateTo)
        external returns (uint256 realToAmount);
}

// Maverick V2: maverickprotocol/v2-common@d91e8714 contracts/interfaces/IMaverickV2Pool.sol L106-L111 (SwapParams),
// L233 (tokenA), L361-L386 (swap: обратный вызов только при непустых data).
interface IMaverickV2Pool {
    struct SwapParams {
        uint256 amount;
        bool tokenAIn;
        bool exactOutput;
        int32 tickLimit;
    }
    function tokenA() external view returns (address);
    function swap(address recipient, SwapParams memory params, bytes calldata data)
        external returns (uint256 amountIn, uint256 amountOut);
}

// iZiSwap (решение 286): izumiFinance/iZiSwap-core@3fe21eba contracts/iZiSwapPool.sol L383-L438 (swapY2X / swapX2Y —
// delegatecall в модуль), contracts/swapX2Y.sol L123-L131 (lowPt = max(lowPt, leftMostPt)), L323-L331 (выход —
// recipient до обратного вызова; оплата x в swapX2YCallback, проверка balanceX() ≥ bx + amountX), swapY2X.sol
// L123-L131, L290-L297 (то же для y и swapY2XCallback); interfaces/IiZiSwapCallback.sol L19-L39. Biswap V3
// (BiswapPoolV3, verified Sourcify exact_match 56/0x76008e4007ab26142dbeafee24d6113de47eb76b, BiswapPoolV3.sol
// L422-L476): те же селекторы swapX2Y 0x857f812f / swapY2X 0x2c481252 и те же обратные вызовы (проба на цепи
// tests/data/izi_2026-11-15), но возвращает три значения (amountX, amountY, accFee) — возврат не читается, поэтому
// в интерфейсе его нет. tokenX < tokenY — фабрики сортируют пару (iZiSwapFactory.sol L101-L104, BiswapFactoryV3.sol
// L116-L119).
interface IIziPool {
    function swapX2Y(address recipient, uint128 amount, int24 lowPt, bytes calldata data) external;
    function swapY2X(address recipient, uint128 amount, int24 highPt, bytes calldata data) external;
}

// Balancer V2 Vault (решение 289; ACSI Finance BSC 0xa82f327bbbf0667356d2935c6532d164b06ceced — Sourcify exact_match,
// исходник = balancer/balancer-deployments@b55dc2b v2/tasks/20210418-vault build-info, 45 из 45 файлов совпали):
// contracts/vault/interfaces/IVault.sol L498 (SwapKind), L513-L518 (swap), L530-L537 (SingleSwap), L623-L628
// (FundManagement). Swaps.sol L56-L104: swap — whenNotPaused, authenticateFor(funds.sender) (вызывающий = sender — без
// relayer), выход пула → _receiveAsset: transferFrom(sender → Vault, amountIn) (AssetTransfersHandler.sol L44-L79; нужен
// approve sender → Vault), _sendAsset → recipient; обратных вызовов к вызывающему нет. GIVEN_IN: limit — минимум выхода.
interface IBalancerVault {
    struct SingleSwap {
        bytes32 poolId;
        uint8 kind;
        address assetIn;
        address assetOut;
        uint256 amount;
        bytes userData;
    }
    struct FundManagement {
        address sender;
        bool fromInternalBalance;
        address payable recipient;
        bool toInternalBalance;
    }
    function swap(SingleSwap memory singleSwap, FundManagement memory funds, uint256 limit, uint256 deadline)
        external
        payable
        returns (uint256);
}

// Liquidity Book V2.1 (решение 290; LFJ BSC, фабрика 0x8e42f2f4101563bf679975178e880fd87d3efd4e, реализации пула
// 0xee5a90098b270596ec35d637b30d908c862c86df и 0x3e30fdae04c08fc20fa2fe0cf55c95d99a9c2d8f — Sourcify exact_match,
// исходник = lfj-gg/joe-v2@v2.1.0): src/interfaces/ILBPair.sol — getTokenX (LBPair.sol L132-L134: аргумент клона),
// swap(bool swapForY, address to) returns (bytes32 amountsOut) (LBPair.sol L483-L557: вход переводится в пул заранее,
// вызывающий не ограничен — только nonReentrant; выход — transfer `to`).
interface ILBPair {
    function getTokenX() external view returns (address);
    function swap(bool swapForY, address to) external returns (bytes32 amountsOut);
}

// Решение 445: Liquidity Book v2.0 (LFJ V2; Arbitrum One — фабрика 0x1886D09C9Ade0c5DB822D85D21678Db67B6c2982,
// официальная документация traderjoe-xyz/joe-docs 61c16665 versioned_docs/version-V2.2/deployment-addresses/arbitrum.md
// L52-L58): lfj-gg/joe-v2@v2.0.0 (07f10212) src/interfaces/ILBPair.sol L148 (tokenX), L206 (swap(bool, address) returns
// (uint256 amountXOut, uint256 amountYOut)); src/LBPair.sol L335-L429 (вход переводится в пару заранее, вызывающий не
// ограничен — только nonReentrant; выход — safeTransfer `to`). Селектор swap(bool,address) — тот же 0x53c059a0, что у
// v2.1; ответ не разбирается.
interface ILBPairV20 {
    function tokenX() external view returns (address);
    function swap(bool swapForY, address to) external returns (uint256 amountXOut, uint256 amountYOut);
}

// Решение 395 (t181, исполнитель v3): пулы внутри общего менеджера — Uniswap V4 и PancakeSwap Infinity (CL, Bin;
// Infinity Stable — пул в крючке пула CL). Обмен идёт только внутри «открытого» менеджера: исполнитель зовёт unlock
// (Uniswap) / lock (Infinity), менеджер вызывает у него unlockCallback / lockAcquired, внутри — swap, затем долг входа
// гасится sync → перевод токена менеджеру → settle, выход забирается take; к концу обратного вызова у вызывающего не
// должно остаться ненулевых дельт (иначе менеджер откатывает всё).
// Uniswap/v4-core@bea7f6d1c11cd5dc4f0d5151ffa37a1c239feee6: src/interfaces/IPoolManager.sol L114 (unlock), L145-L147
// (swap → BalanceDelta = int256), L171 (sync), L179 (take), L183 (settle); src/PoolManager.sol L104-L114 (unlock: зовёт
// IUnlockCallback(msg.sender).unlockCallback, затем NonzeroDeltaCount должен быть 0), L187-L227 (swap: дельта вызывающего
// — _accountPoolBalanceDelta(key, swapDelta, msg.sender), дельты крючка — на адрес крючка), L279-L288, L291-L302,
// L349-L365 (sync / take / settle: оплачено = баланс менеджера − резерв после sync); src/interfaces/callback/
// IUnlockCallback.sol L9; src/types/PoolKey.sol (currency0, currency1, fee uint24, tickSpacing int24, hooks);
// src/types/PoolOperation.sol L19-L26 (SwapParams: amountSpecified < 0 — точный вход); src/interfaces/IExttload.sol L9 и
// src/libraries/TransientStateLibrary.sol L35-L43 (дельта вызывающего = exttload(keccak256(target, currency))).
// Порядок «sync → перевод → settle» и «take» — как в официальной периферии: Uniswap/v4-periphery@0c4e5bd3
// src/base/DeltaResolver.sol L38-L48, L60-L76.
struct V4PoolKey {
    address currency0;
    address currency1;
    uint24 fee;
    int24 tickSpacing;
    address hooks;
}

struct V4SwapParams {
    bool zeroForOne;
    int256 amountSpecified;
    uint160 sqrtPriceLimitX96;
}

interface IV4PoolManager {
    function unlock(bytes calldata data) external returns (bytes memory);
    function swap(V4PoolKey memory key, V4SwapParams memory params, bytes calldata hookData)
        external
        returns (int256 swapDelta);
    function exttload(bytes32 slot) external view returns (bytes32 value);
}

// PancakeSwap Infinity: pancakeswap/infinity-core@3c5ef1be1f71442d8bc60cc6fd11f9c677b714df — src/Vault.sol L74-L86 (lock:
// зовёт ILockCallback(msg.sender).lockAcquired, затем непогашенных дельт быть не должно), L143-L148 (take), L158-L175
// (sync, settle), L238-L253 (_settle: оплачено = баланс − резерв после sync); src/interfaces/IVault.sol L56
// (currencyDelta(settler, currency)); src/interfaces/ILockCallback.sol L9; src/types/PoolKey.sol (currency0, currency1,
// hooks, poolManager, fee uint24, parameters bytes32); src/pool-cl/CLPoolManager.sol L162-L212 (swap: дельта
// вызывающего пишется в Vault на msg.sender — libraries/VaultAppDeltaSettlement.sol L12-L21);
// src/pool-cl/interfaces/ICLPoolManager.sol L150-L154 (SwapParams); src/pool-bin/interfaces/IBinPoolManager.sol L187-L193
// (swap(key, swapForY, int128 amountSpecified < 0 — точный вход, hookData)); src/pool-bin/BinPoolManager.sol L133-L177.
// Порядок «sync → перевод → settle» — pancakeswap/infinity-periphery@56a7a4f5 src/base/DeltaResolver.sol L35-L45.
struct InfPoolKey {
    address currency0;
    address currency1;
    address hooks;
    address poolManager;
    uint24 fee;
    bytes32 parameters;
}

struct InfClSwapParams {
    bool zeroForOne;
    int256 amountSpecified;
    uint160 sqrtPriceLimitX96;
}

interface IInfVault {
    function lock(bytes calldata data) external returns (bytes memory);
    function currencyDelta(address settler, address currency) external view returns (int256);
}

interface IInfClPoolManager {
    function swap(InfPoolKey memory key, InfClSwapParams memory params, bytes calldata hookData)
        external
        returns (int256 delta);
}

interface IInfBinPoolManager {
    function swap(InfPoolKey memory key, bool swapForY, int128 amountSpecified, bytes calldata hookData)
        external
        returns (int256 delta);
}

// Общие для менеджера Uniswap V4 и Vault Infinity (одинаковые подписи и селекторы): sync(Currency) — Currency = address
// в ABI; take(Currency, address, uint256); settle() payable. Ссылки на строки — выше.
interface ISingletonSettle {
    function sync(address currency) external;
    function take(address currency, address to, uint256 amount) external;
    function settle() external payable returns (uint256 paid);
}

// Решение 440: пул Fluid DEX T1 — Instadapp/fluid-contracts-public@404d73f
// contracts/protocols/dex/poolT1/coreModule/core/main.sol L689-L700 (swapIn; селектор 0x2668dfaa — есть в коде пулов
// Arbitrum, deployments/arbitrum/Dex_*.json). to == 0xdEaD — оценка откатом; исполнитель передаёт себя или владельца.
interface IFluidDexT1 {
    function swapIn(bool swap0to1_, uint256 amountIn_, uint256 amountOutMin_, address to_)
        external
        payable
        returns (uint256 amountOut_);
}

// Решение 441 (T433-F2): хранилище Balancer V3 (Vault V3, balancer/balancer-deployments@b55dc2b8 build-info
// v3/tasks/20241204-v3-vault; Arbitrum One — 0xbA1333333333a1BA1108E8412f11850A5C319bA9, output/arbitrum.json).
// IVaultMain.sol L27 unlock (Vault.sol L133-L135: обратный вызов msg.sender.functionCall(data); модификатор transient —
// в конце все дельты должны быть нулевые, иначе BalanceNotSettled), L49 settle (Vault.sol L138-L156: кредит = прирост
// баланса хранилища, не больше подсказки), L60 sendTo (L158-L163: долг + перевод получателю), L74 swap (Vault.sol
// L197-L291: onlyWhenUnlocked, дельты — вызывающему: долг по входу, кредит по выходу; VaultTypes.sol L287-L295
// VaultSwapParams, SwapKind EXACT_IN = 0, L263). Белого списка вызывающих нет.
interface IBalancerV3Vault {
    struct VaultSwapParams {
        uint8 kind;
        address pool;
        address tokenIn;
        address tokenOut;
        uint256 amountGivenRaw;
        uint256 limitRaw;
        bytes userData;
    }
    function unlock(bytes calldata data) external returns (bytes memory result);
    function swap(VaultSwapParams memory vaultSwapParams)
        external
        returns (uint256 amountCalculatedRaw, uint256 amountInRaw, uint256 amountOutRaw);
    function settle(address token, uint256 amountHint) external returns (uint256 credit);
    function sendTo(address token, address to, uint256 amount) external;
    // Решение 474: обмен через буфер ERC4626 хранилища (шаг BALANCER_V3_BUF = 27). IVaultMain.sol L175-L177,
    // VaultTypes.sol L410-L430 (WrappingDirection WRAP = 0 / UNWRAP = 1, BufferWrapOrUnwrapParams; enum — uint8 в ABI);
    // Vault.sol L1139-L1187 (onlyWhenUnlocked, буфер инициализирован, буферы не на паузе, сумма ≥ _MINIMUM_WRAP_AMOUNT;
    // дельты — вызывающему: долг по входу, кредит по выходу; EXACT_IN — выход < limitRaw — откат SwapLimit).
    struct BufferWrapOrUnwrapParams {
        uint8 kind;
        uint8 direction;
        address wrappedToken;
        uint256 amountGivenRaw;
        uint256 limitRaw;
    }
    function erc4626BufferWrapOrUnwrap(BufferWrapOrUnwrapParams memory params)
        external
        returns (uint256 amountCalculatedRaw, uint256 amountInRaw, uint256 amountOutRaw);
}

// Решение 447: ядро Ekubo (Core) — EkuboProtocol/evm-contracts@61d09daa (ядро Arbitrum, broadcast DeployAll.s.sol/42161):
// src/base/FlashAccountant.sol L146-L187 (lock: зовёт у вызывающего locked_6416899205(id) с данными после селектора; в
// конце долгов быть не должно — DebtsNotZeroed), L190-L221 (forward(to): зовёт у расширения forwarded_2374103877 с
// данными после 36 байт), L224-L254 (startPayments: токены словами после селектора), L257-L319 (completePayments:
// оплачено = прирост баланса ядра), L322-L381 (withdraw: записи по 56 байт — токен 20 | получатель 20 | сумма 16);
// src/Core.sol L510-L858 (swap_6269342730, селектор 0x00000000: ключ пула 3 слова + SwapParameters 1 слово; ответ —
// balanceUpdate, stateAfter); src/interfaces/IFlashAccountant.sol L7 (locked_6416899205 — селектор 0x00000000).
// Функции объявлены только ради селекторов (lock 0xf83d08ba, forward 0x101e8952, startPayments 0xf9b6a796,
// completePayments 0x12e103f1, withdraw 0x3ccfd60b): данные этих вызовов — не ABI, их собирает ArbExecutor.
interface IEkuboCore {
    function lock() external;
    function forward(address to) external;
    function startPayments() external;
    function completePayments() external;
    function withdraw() external;
}

// Решение 484: пул Wombat Exchange — wombat-exchange/v1-core 2ac067b8 contracts/wombat-core/pool/PoolV2.sol L843-L850
// (swap: вход — safeTransferFrom(msg.sender) по разрешению пулу, выход — получателю `to`; ответ — выход и haircut).
// Развёрнутые реализации Arbitrum (HighCovRatioFeePoolV2 / DynamicPoolV2) — та же подпись (Sourcify; сверка —
// docs/sources/wombat_arbitrum/README.md).
interface IWombatPool {
    function swap(
        address fromToken,
        address toToken,
        uint256 fromAmount,
        uint256 minimumToAmount,
        address to,
        uint256 deadline
    ) external returns (uint256 actualToAmount, uint256 haircut);
}
// Решение 486: учётный контракт Shell Protocol (Ocean) — Shell-Protocol/Shell-Protocol@aaa0ccf src/ocean/Interactions.sol
// (struct Interaction, enum InteractionType: WrapErc20 = 0, UnwrapErc20 = 1, ComputeOutputAmount = 7, UnwrapEther = 8),
// src/ocean/Ocean.sol L271-L297 (doMultipleInteractions, payable; value — обёртка нативной монеты учёта); Ocean v3
// Arbitrum (Sourcify 42161 0x96B4f4E401cCD70Ec850C1CF8b405Ad58FD5fB7a, exact_match) — те же подписи (Ocean.sol
// L190-L201, Interactions.sol). Селектор doMultipleInteractions((bytes32,uint256,uint256,uint256,bytes32)[],uint256[])
// = 0xd44f70b4.
interface IShellOcean {
    struct Interaction {
        bytes32 interactionTypeAndAddress;
        uint256 inputToken;
        uint256 outputToken;
        uint256 specifiedAmount;
        bytes32 metadata;
    }

    function doMultipleInteractions(Interaction[] calldata interactions, uint256[] calldata ids)
        external
        payable
        returns (
            uint256[] memory burnIds,
            uint256[] memory burnAmounts,
            uint256[] memory mintIds,
            uint256[] memory mintAmounts
        );
}
