//! Файл: venues/bsc_dex/pools/exec_kind.rs
//! Что делает: вид шага контракта-исполнителя `ArbExecutor` для каждого вида пула (решение 122, часть F; таблица видов
//! шагов — docs/notes/smart-router-design-2026-11-03.md, часть X). Это пометка на стороне данных/котировки: каким
//! кодировщиком шага (exec/executor/*, часть X) исполнитель вызовет пул этого вида. Сами кодировщики и контракт —
//! часть X; здесь только соответствие «dex.kind → вид шага» и правило «исполнимо через исполнитель»: вид шага есть И
//! задан адрес исполнителя exec.executor_address [bsc_dex]. Пока адрес не задан — семейство остаётся сигналом (как
//! раньше, по family.tradable). Списка DEX в коде нет: соответствие — по перечню модулей адаптера (dex.kind).
//! Кто вызывает: pools/families.rs (make — поле Family.executor_kind), quote/* и exec/* (via_executor — части R и X).
//! От чего зависит: ctx.rs, pools/families.rs (Family), exec/executor/live_proof.rs (describe — решение 400).
//! Настройки: exec.executor_address [bsc_dex] (заводит часть X; не задана — исполнителя нет); через exec_callback.rs —
//! dex.v3_swap_callback [DEX] (ревью 281: обратного вызова пула нет в коде исполнителя — пулы DEX без пути исполнения).
//!
//! PancakeSwap Infinity (CL, Bin и Infinity Stable — пул в крючке CL, решение «122 (Infinity Stable)») и Uniswap V4
//! (общий PoolManager) исполняются своим Universal Router (часть R2); решение 395 (t181): у них есть и шаг исполнителя
//! v3 — UNI_V4 / INF_CL / INF_BIN (пул внутри менеджера: unlock / lock + обратный вызов, contracts/ArbExecutor.sol), чтобы
//! маршрут, где они смешаны с пулами других DEX (сплит, хопы), шёл одной транзакцией; одиночный путь своего Universal
//! Router не меняется (graph_executor.rs ur_form). Шаг на хопе стороны BNB, когда BNB — монета сделки, не
//! берётся (singleton; хоп, не касающийся BNB, — берётся: executeNative с шагом менеджера). KyberSwap Elastic вида шага пока не получает: его `swap(recipient, swapQty, isToken0,
//! limitSqrtP, data)` (ks-elastic-sc IPoolActions.sol) не совпадает с V3 `swap(recipient, zeroForOne,
//! amountSpecified, sqrtPriceLimitX96, data)` — вопрос Тиграну (отчёт части F).

use super::super::ctx::Ctx;
use super::super::exec::executor::code::Version;
use super::exec_callback;
use super::families::{Family, Proto};

/// Вид шага исполнителя (по официальному интерфейсу контракта DEX; кодировщик — exec/executor/*, часть X).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExecKind {
    /// Пара V2 и её форки: перевод входа в пару + `pair.swap` по резервам и комиссии пары.
    V2,
    /// V3-подобный пул: `pool.swap(recipient, zeroForOne, +amountIn, предел цены, data)` + обратный вызов
    /// (PancakeSwap V3, Uniswap V3, форки V3, Topaz Slipstream).
    V3,
    /// Algebra (THENA Fusion): `pool.swap(...)` + `algebraSwapCallback`.
    Algebra,
    /// Solidly (THENA V1, Topaz v2; volatile и stable): `pair.getAmountOut` в контракте, перевод, `pair.swap`.
    Solidly,
    /// Curve StableSwap — только Ellipsis (плоский пул Curve): `approve` пулу + `exchange(int128 i, int128 j, dx, 0)`.
    /// PancakeSwap Stable Swap, Nerve (Saddle), Lista — другие подписи обмена, не этот вид (техдолг, docs/ROADMAP.md;
    /// contracts/README.md).
    Curve,
    /// DODO V2 (DVM/DPP/DSP): перевод в пул + `sellBase(to)` / `sellQuote(to)`.
    Dodo,
    /// WooFi (WooPPV2): перевод в пул + `swap(...)`.
    Woo,
    /// Maverick V2: `pool.swap(recipient, SwapParams{amount, tokenAIn, exactOutput=false, tickLimit}, data)` + обратный
    /// вызов `maverickV2SwapCallback`; направление tokenA и предел тика контракт берёт сам (contracts/ArbExecutor.sol).
    Maverick,
    /// StableSwap с индексами uint256 — PancakeSwap Stable Swap и Lista: `approve` пулу + `exchange(uint256 i,
    /// uint256 j, dx, 0)` (решение 122 X, стейблсвапы).
    StableU256,
    /// Saddle — Nerve: `approve` пулу + `swap(uint8 i, uint8 j, dx, 0, срок)` (решение 122 X, стейблсвапы).
    Saddle,
    /// Nomiswap Stable (решение 284): перевод входа в пару + `pair.getAmountOut(tokenIn, вход)` в контракте + `pair.swap`
    /// (как SOLIDLY, порядок аргументов getAmountOut обратный). Вид шага 12 есть ТОЛЬКО у исполнителя v2
    /// (exec_callback.rs: у v1 — без пути исполнения с причиной).
    NomiStable,
    /// Metric OMM (решение 285): `pool.swap(to, zeroForOne, +вход, предел, «01», «»)` + обратный вызов
    /// `metricOmmSwapCallback`; zeroForOne — в data (порядок токенов пула не по адресу). Только исполнитель v2.
    Metric,
    /// iZiSwap / Biswap V3 (решение 286): `swapX2Y(to, вход, int24.min, «»)` при tokenIn < tokenOut, иначе
    /// `swapY2X(to, вход, int24.max, «»)` + обратный вызов swapX2YCallback / swapY2XCallback → _pay. Только исполнитель v2.
    Izi,
    /// KyberSwap Classic DMM (решение 287): перевод входа в пул + выход по getTradeInfo() пула (виртуальные резервы и
    /// комиссия блока, формула DMMLibrary.getAmountOut) в контракте + `pool.swap` (подпись V2). Только исполнитель v2.
    KyberDmm,
    /// SmarDex (решение 288): `pair.swap(to, zeroForOne по порядку адресов, +вход, «»)` + обратный вызов
    /// `smardexSwapCallback` → _pay. Только исполнитель v2.
    Smardex,
    /// Balancer V2 (решение 289): approve хранилищу ровно на шаг + `Vault.swap(GIVEN_IN, poolId, вход, получатель)`;
    /// пул шага — хранилище, data = abi.encode(bytes32 poolId). Только исполнитель v2.
    Balancer,
    /// Liquidity Book V2.1 (решение 290): перевод входа в пул + `pair.swap(swapForY = (getTokenX() пула == вход), to)`;
    /// без обратного вызова, data пусто. Только исполнитель v2.
    Lb,
    /// Решение 441 (T433-F2): пул Balancer V3 — `Vault.unlock` → `balancerV3Callback` исполнителя → `Vault.swap(EXACT_IN,
    /// пул, вход, выход, сумма, предел 1)` → перевод входа хранилищу + `settle`, `sendTo` выхода получателю шага; пул шага —
    /// хранилище, data = abi.encode(address пула). Только исполнитель v4.
    BalancerV3,
    /// Liquidity Book v2.0 (решение 445): перевод входа в пул + `pair.swap(swapForY = (tokenX() пула == вход), to)`
    /// (у пары v2.0 нет getTokenX — шаг LB v2.1 откатился бы); без обратного вызова, data пусто. Шаг 24 — в исходнике
    /// contracts/ArbExecutor.sol; в коде исполнителя — только сборка v4 (exec_callback.rs since; решение 457 — out/).
    Lb20,
    /// Решение 395 (t181): пул Uniswap V4 внутри PoolManager — `unlock` → `unlockCallback` → `swap(ключ, …)` → `sync` /
    /// перевод / `settle` долга, `take` выхода; data = abi.encode(PoolKey). Только исполнитель v3.
    UniV4,
    /// Решение 395: пул PancakeSwap Infinity CL (и Infinity Stable — пул в крючке пула CL) — `Vault.lock` → `lockAcquired`
    /// → `CLPoolManager.swap(ключ, …)` → `sync` / перевод / `settle`, `take`; data = abi.encode(PoolKey Infinity). Только v3.
    InfCl,
    /// Решение 395: пул PancakeSwap Infinity Bin — как InfCl, обмен `BinPoolManager.swap(ключ, swapForY, −вход, «»)`.
    /// Только исполнитель v3.
    InfBin,
    /// Решение 440: пул Fluid DEX T1 — approve пулу ровно на шаг + `pool.swapIn(swap0to1, вход, 0, получатель)` (вход
    /// пул списывает transferFrom исполнителя в обратном вызове слоя ликвидности); нативная монета пула — через обёртку
    /// (withdraw → swapIn{value} / приход от слоя ликвидности → deposit); data = abi.encode(bool swap0to1, uint8 native,
    /// address liquidity). Только исполнитель v4.
    Fluid,
    /// Решение 447: пул Ekubo в ядре Core — `Core.lock()` → `locked_6416899205` → `swap_6269342730` (у пула с
    /// MEVCapture — `forward(расширение)`) → `startPayments` / перевод / `completePayments` долга, `withdraw` выхода;
    /// решение 454: data = abi.encode(bytes32 config, bool native, bool forward) — native: token0 ключа нативный ETH
    /// (его сторона шага — обёртка wnative), forward: обмен через Core.forward(расширение) у MEVCapture
    /// (contracts/ArbExecutor.sol _ekubo). Есть только у кода исполнителя с шагом EKUBO (exec_callback.rs EKUBO_MARK).
    Ekubo,
    /// Решение 470 (T470-06): пул Aero Basic — как SOLIDLY (перевод входа в пул, swap подписи V2), но выход —
    /// getAmountOutWithTotalFee пула (комиссия с MEV-налогом этой транзакции, Dromos-Labs/metadex-public 79c8d5b6
    /// V3/src/pools/Pool.sol L548-L555). Шаг 26 — только исполнитель v5 (exec_callback.rs since). Вид семейства
    /// aero_basic остаётся SOLIDLY (of); шаг AERO_BASIC выбирает for_version по версии исполнителя кошелька.
    AeroBasic,
    /// Решение 472 (T472-08): пул Curve с нативным ETH (Plain*ETH / Plain2ETHEMA фабрики Curve) — как CURVE, но монета
    /// пары нативная: обёртка withdraw → exchange{value} / приход ETH от пула → deposit (contracts/ArbExecutor.sol
    /// _curveEth). Шаг 28 — только исполнитель v5 (exec_callback.rs since). Вид семейства остаётся CURVE (of); шаг
    /// CURVE_ETH выбирает exec/executor/entry.rs, когда монета пары — нативная монета пула (StableData.native).
    CurveEth,
    /// Решение 484: пул Wombat Exchange — approve пулу ровно на шаг + `pool.swap(tokenIn, tokenOut, вход, 1, to,
    /// block.timestamp)`: пул списывает вход transferFrom(исполнитель → актив входа) и переводит выход `to` из актива
    /// выхода (wombat-exchange/v1-core 2ac067b8 PoolV2.sol L843-L876). Шаг 30 — только исполнитель v5 (exec_callback.rs
    /// since).
    Wombat,
    /// Решение 485: скважина Basin — approve скважине ровно на шаг + `swapFrom(tokenIn, tokenOut, сумма, 0, получатель,
    /// block.timestamp)` (BeanstalkFarms/Basin@ecf69233 src/Well.sol L207-L217: вход скважина списывает
    /// transferFrom(исполнитель)); data пусто. Шаг 29 — только исполнитель v5 (exec_callback.rs since).
    Basin,
    /// Решение 486: пул Shell Protocol (Proteus) — обмен через учётный контракт Ocean: `doMultipleInteractions` с
    /// wrapErc20 (вход; у нативной монеты учёта — обёртка withdraw и value вызова), computeOutputAmount пула и
    /// unwrapErc20 / unwrapEther (выход; нативная монета — ETH от Ocean → deposit в обёртку) с GET_BALANCE_DELTA
    /// (contracts/ArbExecutor.sol _shell). data = abi.encode(address primitive, uint8 native, uint8 decimalsIn)
    /// (primitive — пул LiquidityPoolProxy, цель computeOutputAmount; пул шага — учёт Ocean). Шаг 31 — только
    /// исполнитель v5 (exec_callback.rs since).
    Shell,
}

impl ExecKind {
    /// Имя вида для журнала и интерфейса (как в таблице видов шагов). Ошибок нет.
    pub fn name(self) -> &'static str {
        match self {
            ExecKind::V2 => "V2",
            ExecKind::V3 => "V3",
            ExecKind::Algebra => "ALGEBRA",
            ExecKind::Solidly => "SOLIDLY",
            ExecKind::Curve => "CURVE",
            ExecKind::Dodo => "DODO",
            ExecKind::Woo => "WOO",
            ExecKind::Maverick => "MAVERICK",
            ExecKind::StableU256 => "STABLE_U256",
            ExecKind::Saddle => "SADDLE",
            ExecKind::NomiStable => "NOMI_STABLE",
            ExecKind::Metric => "METRIC",
            ExecKind::Izi => "IZI",
            ExecKind::KyberDmm => "KYBER_DMM",
            ExecKind::Smardex => "SMARDEX",
            ExecKind::Balancer => "BALANCER",
            ExecKind::Lb => "LB",
            ExecKind::BalancerV3 => "BALANCER_V3",
            ExecKind::Lb20 => "LB20",
            ExecKind::UniV4 => "UNI_V4",
            ExecKind::InfCl => "INF_CL",
            ExecKind::InfBin => "INF_BIN",
            ExecKind::Fluid => "FLUID",
            ExecKind::Ekubo => "EKUBO",
            ExecKind::AeroBasic => "AERO_BASIC",
            ExecKind::CurveEth => "CURVE_ETH",
            ExecKind::Wombat => "WOMBAT",
            ExecKind::Basin => "BASIN",
            ExecKind::Shell => "SHELL",
        }
    }

    /// Решение 395: шаг пула внутри общего менеджера (Uniswap V4 / PancakeSwap Infinity). На хопе стороны BNB (BNB —
    /// монета сделки) такой шаг не берётся (exec/native_exec.rs, check/native_probe.rs — как до решения 395); хоп, не
    /// касающийся BNB, — берётся (executeNative с шагом менеджера). Ошибок нет.
    pub fn singleton(self) -> bool {
        matches!(self, ExecKind::UniV4 | ExecKind::InfCl | ExecKind::InfBin)
    }
}

/// Вид шага исполнителя по варианту dex.kind. Принимает: вариант dex.kind. Возвращает: вид шага; None — пулы этого
/// вида исполнитель не вызывает (Universal Router своей фабрики, KyberSwap Elastic — см. шапку, неизвестный вид).
/// CURVE — curve_stableswap (решение 122 F, 2), MAVERICK — maverick_v2 (решение 122 F, Maverick V2). WooFi (woofi_v2,
/// решение 122 F, 3) шаг WOO пока НЕ получает: своей котировки нет (развёрнутый код закрыт), шаг включается только
/// после сухой сделки на живом пуле (сверка подписи swap развёрнутой реализации, селектор 0x7dc20382 в байткоде).
/// PancakeSwap / Lista StableSwap (exchange с индексами uint256) — STABLE_U256, Saddle / Nerve (swap(uint8, uint8, …,
/// deadline)) — SADDLE (решение 122 X, стейблсвапы). V2-форк без роутера (v2_fork_executor, решение 280) — V2. Ошибок нет.
pub fn of(kind: &str) -> Option<ExecKind> {
    Some(match kind {
        "pancakeswap_v2"
        | "uniswap_v2"
        | "v2_fork"
        | "v2_whiteswap"
        | "v2_fork_executor"
        | "v2_fork_executor_create2"
        | "v2_diamond_router" | "v2_diamond_create"
        | "v2_fork_executor_create2_live_fee" | "candyswap_v2" => ExecKind::V2,
        // Решение 483: Ramses CL — swap(recipient, zeroForOne, amountSpecified, sqrtPriceLimitX96, data) как у V3
        // (RamsesV2Pool.sol L944), обратный вызов ramsesV2SwapCallback (L1238) — проверка dex.v3_swap_callback
        // (exec_callback.rs); Ramses CL V2 — тот же swap и uniswapV3SwapCallback (RamsesV3Pool.sol L436-L442, L662).
        "pancakeswap_v3"
        | "uniswap_v3"
        | "v3_fork"
        | "v3_fork_pancake"
        | "slipstream"
        | "slipstream_aero"
        | "slipstream_aero_metaquoter"
        | "slipstream_dynamic_fee"
        | "v3_fork_ramses"
        | "slipstream_ramses"
        | "v3_fork_uniswap_lm" => ExecKind::V3,
        "thena_fusion" => ExecKind::Algebra,
        // Решение 188: swap(recipient, zeroToOne, amountRequired, limitSqrtPrice, data) и algebraSwapCallback у
        // Integral — те же (integral-core 1.2.10 AlgebraPool.sol L246-L252, IAlgebraSwapCallback).
        "algebra_integral"
        | "algebra_integral_base_plugin_v1"
        | "algebra_integral_hydrex"
        | "algebra_integral_quickswap"
        | "algebra_integral_nami" | "algebra_integral_supernova" => ExecKind::Algebra,
        // Решение 444: Algebra V1.9 с направленными комиссиями (Camelot V3) — swap(recipient, zeroToOne,
        // amountRequired, limitSqrtPrice, data) и algebraSwapCallback(int256, int256, bytes) те же (cryptoalgebra/
        // AlgebraV1.9 a45417ed AlgebraPool.sol:604-610, IAlgebraSwapCallback.sol:18-22) — шаг ALGEBRA, исполнитель тот же.
        "algebra_dir_fee" => ExecKind::Algebra,
        // Решение 483: пара Ramses Legacy — getReserves (uint112, Pair.sol L125), getAmountOut(uint256, address) L452 (уже
        // с −1) и swap(uint256, uint256, address, bytes) L308; вход = баланс − резерв (L339-L340) — шаг SOLIDLY.
        "solidly_v2" | "solidly_v1" | "solidly_v1_ramses" | "leetswap_v2" => ExecKind::Solidly,
        // Решение 443: пара Camelot V2 — тот же шаг SOLIDLY: getReserves (первые два слова — резервы), getAmountOut(uint256,
        // address) и swap(uint256, uint256, address, bytes) есть у CamelotPair (Pair.sol L46, L372, L228 — verified-исходник
        // фабрики Arbitrum 0x6EcC…A652, math/camelot_v2.rs); вход = баланс − резерв — так и считает swap пары (L276-L277).
        "camelot_v2" => ExecKind::Solidly,
        // Решение 470: пул Aero Basic — тот же шаг SOLIDLY: getReserves (три uint256, Pool.sol L530), getAmountOut(uint256,
        // address) L537 (комиссия getBaseFee без MEV-налога — налог исключён правилом aero_mev.rs) и swap(uint256, uint256,
        // address, bytes) L143 (metadex-public 1.1.0); вход = баланс − резерв — так и считает swap пула (L161-L162).
        // T470-06: у исполнителя v5 — шаг AERO_BASIC (for_version), налог > 0 исполняется.
        "aero_basic" => ExecKind::Solidly,
        "dodo_dvm" | "dodo_dpp" | "dodo_dsp" => ExecKind::Dodo,
        // Решение 275: Curve StableSwap-NG — exchange(int128, int128, dx, min_dy) с receiver = msg.sender по умолчанию
        // (curvefi/stableswap-ng 3332cd4, CurveStableSwapNG.vy L504-L531): тот же шаг CURVE.
        // Решение 472: Curve StableSwap 2021 (2pool Arbitrum, Plain*Basic фабрики) — exchange(int128, int128, dx, min_dy):
        // у Plain*Basic `_receiver = msg.sender` по умолчанию (curve-factory f389123 Plain2Basic.vy L530-L536), у 2pool —
        // перегрузка из 4 аргументов в официальном ABI curvefi/curve-js ab3f1c2 src/constants/abis/2pool/swap.json.
        // Метапул фабрики (MetaUSD.vy f389123 L606-L612) — та же подпись exchange, монеты — ERC-20 (монета и LP-токен).
        // Решение 481: пул ETH/wstETH (curve_stableswap_oracle) — exchange(int128, int128, dx, min_dy) payable
        // (docs/sources/curve_arbitrum/0x6eB2…7d80.vy L444); монета пары — всегда нативный ETH пула, поэтому шаг —
        // CURVE_ETH (exec/executor/entry.rs stable → curve_eth, только исполнитель v5).
        "curve_stableswap"
        | "curve_stableswap_ng"
        | "curve_stableswap_plain"
        | "curve_stableswap_meta"
        | "curve_stableswap_oracle" => ExecKind::Curve,
        "maverick_v2" => ExecKind::Maverick,
        "pancake_stableswap" | "lista_stableswap" | "lista_stableswap_proxy" => {
            ExecKind::StableU256
        }
        // Решение 442: Curve Twocrypto-NG / Tricrypto-NG — exchange(uint256 i, uint256 j, dx, min_dy) с receiver =
        // msg.sender по умолчанию; вход — transferFrom(msg.sender) (twocrypto-ng 89c8dfb CurveTwocryptoOptimized.vy
        // L364-L404, L286-L335; tricrypto-ng ecaa816 CurveTricryptoOptimizedWETH.vy L410-L442, L313-L379 — use_eth =
        // False: монета ERC-20, у WETH-монеты пул отдаёт WETH): тот же шаг STABLE_U256 (без нового кода исполнителя).
        "curve_twocrypto_ng" | "curve_tricrypto_ng" => ExecKind::StableU256,
        // Решение 453: crypto v1 — exchange(uint256 i, uint256 j, dx, min_dy) с умолчаниями use_eth = False и (у
        // фабрики v1) receiver = msg.sender — тот же селектор (curve-factory-crypto 7ed1011 CurveCryptoSwap2ETH.vy
        // L859-L866: вход transferFrom(sender) L709-L716, выход transfer(receiver) L733-L738; curve-crypto-contract
        // a0f8401 «2021-09-13. Arbitrum/CryptoSwap.vy» L591-L686: transferFrom(msg.sender) L614, transfer(msg.sender)
        // L669) — шаг STABLE_U256, без нового кода исполнителя.
        "curve_crypto_v1" | "curve_tricrypto_v1" | "curve_crypto2_v1" => ExecKind::StableU256,
        "saddle_stableswap" => ExecKind::Saddle,
        // Решение 284: swap пары — подпись V2, выход — getAmountOut(address, uint256) самой пары (шаг 12, только v2).
        "nomiswap_stable" => ExecKind::NomiStable,
        // Решение 285: любой контракт может звать swap пула Metric (проба на цепи — без белого списка у проверенных
        // пулов; пул с белым списком откатит и квотер — лесенки и котировки у него не будет). Шаг 13, только v2.
        "metric_omm" => ExecKind::Metric,
        // Решение 286: swap любым контрактом (проба на цепи — три фабрики, tests/data/izi_2026-11-15). Шаг 14, только v2.
        "iziswap" | "biswap_v3" => ExecKind::Izi,
        // Решение 287: swap пула — подпись V2 без белого списка (DMMPool.sol L162-L210: проверок вызывающего нет); шаг 15, только v2.
        "kyber_dmm" => ExecKind::KyberDmm,
        // Решение 288: swap пары без белого списка (SmardexPair.sol L155-L319: только lock и «to» не токен пары); шаг 16, v2.
        "smardex" | "smardex_exact_equality" => ExecKind::Smardex,
        // Решение 289: Vault.swap — вызывающий = sender, без relayer и белого списка (Swaps.sol L56-L104: whenNotPaused,
        // authenticateFor(funds.sender)); проба на цепи — tests/data/balancer_acsi_2026-11-15/exec_sim.json; шаг 17, v2.
        "balancer_v2" => ExecKind::Balancer,
        // Решение 290: LBPair.swap(swapForY, to) — вызывающий не ограничен (nonReentrant), вход = баланс − резерв
        // (LBPair.sol L483-L490, BinHelper.sol L290-L306); проба на цепи — tests/data/lb_v21_2026-11-15/exec_state.json;
        // шаг 18, v2.
        "lb_v21" | "lb_v21_clone" => ExecKind::Lb,
        // Решение 441: Vault V3 — unlock / swap / settle / sendTo любым контрактом (Vault.sol 20241204-v3-vault L133-L163,
        // L197-L291: onlyWhenUnlocked, без белого списка); шаг 23, только исполнитель v4 (EVM-проба — после записи с узла).
        "balancer_v3" => ExecKind::BalancerV3,
        // Решение 445: LBPair v2.0 swap(swapForY, to) — вызывающий не ограничен (nonReentrant), вход = баланс − резерв −
        // накопленные комиссии (lfj-gg/joe-v2@v2.0.0 src/LBPair.sol L335-L347, TokenHelper.sol L61-L71); шаг 24 (LB20).
        "lb_v20" => ExecKind::Lb20,
        // Решение 446: LB V2.2 — тот же swap(swapForY, to) и getTokenX() (lfj-gg/joe-v2@v2.2.0 src/LBPair.sol L134,
        // L493-L579; вызывающий не ограничен), шаг 18 без изменений исполнителя.
        "lb_v22" => ExecKind::Lb,
        // Решение 395 (t181): пулы внутри менеджеров — шаги v3. Uniswap V4: PoolManager.unlock / swap / sync / settle /
        // take (Uniswap/v4-core@bea7f6d1 src/PoolManager.sol L104-L114, L187-L227, L279-L302); Infinity CL и Bin:
        // Vault.lock / sync / settle / take + CLPoolManager.swap / BinPoolManager.swap (pancakeswap/infinity-core@3c5ef1be
        // src/Vault.sol L74-L86, L143-L175, src/pool-cl/CLPoolManager.sol L162-L212, src/pool-bin/BinPoolManager.sol
        // L133-L177). Infinity Stable — пул в крючке пула CL, обмен — тот же CL_SWAP с ключом пула (как у Universal
        // Router, решение «122 (Infinity Stable)»). EVM-тест с официальным кодом менеджеров — contracts/evmtest/singleton.mjs.
        "uniswap_v4" => ExecKind::UniV4,
        "pancakeswap_infinity_cl" | "pancake_infinity_stable" => ExecKind::InfCl,
        "pancakeswap_infinity_bin" => ExecKind::InfBin,
        // Решение 440: swapIn пула без белого списка вызывающих (poolT1 main.sol L51-L58, L689-L700: пауза и
        // повторный вход — общие для всех); шаг 22, только v4.
        "fluid_t1" => ExecKind::Fluid,
        // Решение 447: Core.lock / swap / forward / startPayments / completePayments / withdraw (EkuboProtocol/evm-contracts
        // @61d09daa src/base/FlashAccountant.sol L146-L187, L190-L221, L224-L381; src/Core.sol L510-L858). Шаг 25 (решение 456).
        "ekubo" => ExecKind::Ekubo,
        // Решение 484: PoolV2.swap — вызывающий не ограничен (nonReentrant, whenNotPaused, срок, пауза актива входа;
        // PoolV2.sol L843-L876), вход — safeTransferFrom(msg.sender) по разрешению пулу; шаг 30, только v5.
        "wombat" => ExecKind::Wombat,
        // Решение 485: swapFrom скважины без белого списка вызывающих (Well.sol L207-L217: nonReentrant и срок — общие).
        // Адаптер Fraxswap реализует тот же wire ABI swapFrom. Адрес шага выбирает executor/entry.
        "memebox_v5" | "basin_well" | "fraxswap_twamm_v5" | "oasisswap_v2_v5" | "ethervista_v5" => ExecKind::Basin,
        // Решение 486: Ocean.doMultipleInteractions — вызывающий = пользователь, без белого списка (Ocean.sol aaa0ccf
        // L271-L297, v3 Sourcify 0x96B4f4E4… L190-L201); computeOutputAmount пула — только от Ocean (onlyOcean), учёт
        // передаёт адрес пользователя (LiquidityPool.sol L135). Шаг 31, только исполнитель v5.
        "shell_proteus" => ExecKind::Shell,
        _ => return None,
    })
}

/// Решение 470 (T470-06): вид шага пула с учётом версии исполнителя кошелька. Пул Aero Basic (Proto::AeroBasic) у
/// сверенного исполнителя не ниже since(AERO_BASIC) (v5) — AERO_BASIC (выход getAmountOutWithTotalFee, MEV-налог
/// исполняется); иначе — вид семейства (SOLIDLY: налог > 0 — отказ перед отправкой, pools/aero_mev.rs fresh).
/// Принимает: вид, сверенную версию исполнителя (None — не сверена). Ошибок нет.
pub fn for_version(f: &Family, ver: Option<Version>) -> Option<ExecKind> {
    let aero = f.proto == Proto::AeroBasic && f.executor_kind == Some(ExecKind::Solidly);
    if aero && ver.is_some_and(|v| v >= exec_callback::since(ExecKind::AeroBasic)) {
        return Some(ExecKind::AeroBasic);
    }
    f.executor_kind
}

/// Решение 470: то же по исполнителю кошелька окружения (снимок без замка, Ctx::exec_info). Ошибок нет.
pub fn for_wallet(ctx: &Ctx, f: &Family) -> Option<ExecKind> {
    for_version(f, ctx.exec_info().and_then(|(_, v)| v))
}

/// Задан ли адрес контракта-исполнителя у кошелька окружения (решение 132: счёт или, без счёта, строка «пробы»).
/// Ошибок нет: не задан или не читается — нет.
pub fn executor_set(ctx: &Ctx) -> bool {
    matches!(ctx.executor(), Ok(Some(_)))
}

/// Исполнимы ли пулы этого вида через контракт-исполнитель: у вида есть шаг исполнителя И задан
/// exec.executor_address (и обратный вызов dex.v3_swap_callback, если задан, есть у исполнителя — exec_callback.rs).
/// Без адреса — false: семейство остаётся сигналом (если у него нет своего пути —
/// family.tradable). Принимает: окружение, вид пула. Ошибок нет.
pub fn via_executor(ctx: &Ctx, f: &Family) -> bool {
    if f.executor_kind.is_none() {
        return false;
    }
    // Решение 401 (ревью t186, Н1): адрес и версия исполнителя — одно чтение снимка кошельков без замка (Ctx::exec_info;
    // раньше — два чтения записи кошелька под замком с её копией: executor_set и exec_callback::step_missing).
    let Some((_, ver)) = ctx.exec_info() else {
        return false;
    };
    // Ревью 281 (В4): у DEX с dex.v3_swap_callback обратный вызов должен быть в коде исполнителя (exec_callback.rs).
    exec_callback::usable(ctx, f, ver)
}

/// Строка для интерфейса (список DEX, решение 122 F): как исполняются пулы этого вида. Решение 395: у вида со своим
/// Universal Router и шагом исполнителя (V4 / Infinity) — приставка «Universal Router своей DEX — как раньше»: причина
/// «без пути исполнения» относится только к исполнителю. Ошибок нет.
pub fn describe(ctx: &Ctx, f: &Family) -> String {
    let base = describe_exec(ctx, f);
    if f.tradable && !f.fork && f.executor_kind.is_some() {
        format!("Universal Router своей DEX — как раньше; {base}")
    } else {
        base
    }
}

/// Решение 461: что приняла запись приёмки версии исполнителя кошелька (не сверена — v3, как pool_gate.rs) в сети
/// площадки (dex.evm_chain_id). Ошибок нет.
fn wallet_listing(ctx: &Ctx) -> String {
    use super::super::exec::executor::live_proof;
    let ver = ctx.exec_info().and_then(|(_, v)| v).unwrap_or(Version::V3);
    live_proof::listing_for(ver, live_proof::chain_of(ctx))
}

/// Часть describe про исполнитель. Ошибок нет.
fn describe_exec(ctx: &Ctx, f: &Family) -> String {
    if let (Some(k), Some(why)) = (f.executor_kind, exec_callback::missing(ctx, f)) {
        return format!("исполнитель: шаг {} — без пути исполнения: {why}", k.name());
    }
    match (f.executor_kind, executor_set(ctx)) {
        // Решение 400: у шага пула менеджера — классы, принятые записью симуляции на живых пулах; решение 401 — и пул
        // непринятого класса, прошедший пробу через исполнитель кошелька (отвергнутый пробой — нет).
        (Some(k), true) if k.singleton() => format!(
            "исполнитель: шаг {} — исполнимо через ArbExecutor в принятых классах или после пробы пула через \
             исполнитель кошелька (решение 401; {})",
            k.name(),
            wallet_listing(ctx)
        ),
        (Some(k), true) => format!(
            "исполнитель: шаг {} — исполнимо через ArbExecutor",
            k.name()
        ),
        (Some(k), false) => format!(
            "исполнитель: шаг {} — включится, когда у кошелька развёрнут исполнитель (экран «Маппинг кошельков»)",
            k.name()
        ),
        (None, _) => {
            "исполнитель: шага нет (Universal Router своей фабрики или вид без шага)".into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::of;
    use crate::settings::catalog::dex::DEX_KINDS;

    /// Каждый вариант каталога либо получает вид шага, либо явно его не имеет (Elastic / WooFi / «не поддерживается»;
    /// решение 395: V4 и Infinity — шаги v3).
    #[test]
    fn every_kind_is_decided() {
        let none = [
            "memebox",
            "ring_few_v2",
            "fraxswap_twamm",
            "fraxswap_twamm_router",
            "v2_fork_router_create2",
            "v2_deltaswap_router",
            "oasisswap_v2_router",
            "ethervista_native",
            "base_native_v3",
            "base_native_v3_module",
            "solidly_v3_module",
            "solidly_v3", // отдельный модуль исполнения, без шага в ArbExecutor V5
            "kyber_elastic",
            "kyber_elastic_v1",
            "woofi_v2",
            "lunarbase",
            "baibai",
            "baibai_module",
            "lunarbase_module",
            "elfomofi",
            "elfomofi_module",
            "unsupported",
        ];
        for k in DEX_KINDS {
            assert_eq!(of(k).is_none(), none.contains(k), "{k}");
        }
    }
}
