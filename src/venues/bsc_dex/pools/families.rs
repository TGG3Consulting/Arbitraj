//! Файл: venues/bsc_dex/pools/families.rs
//! Что делает: DEX и виды пулов из настроек (решение 58). Строка таблицы сопоставления — id DEX у индексатора
//! (id области настроек «DEX»), её вид — настройка dex.kind: PancakeSwap V2 / V3 / Infinity CL / Infinity Bin,
//! Uniswap V2 / V3 / V4 (их пулы исполняет Universal Router своей фабрики — graphable и tradable, не форк), V2-форк
//! (Biswap и прочие — graphable, tradable и fork: исполняется СВОИМ роутером бандлом, решения 59, 82, фаза 80.9b),
//! V2-форк без известного роутера (v2_fork_executor, решение 280: в графе и сигналах, исполнение — шаг V2 исполнителя),
//! V3-форки (решение 122 F: в графе и сигналах — математика и чтение как у V3 своего ABI; исполнение — через
//! контракт-исполнитель) или «не поддерживается». Списка DEX в коде нет: виды — перечень модулей
//! адаптера, а какие DEX каким видом считать и их адреса — вводит Тигран. Подключена ли DEX (classify): вид с путём
//! исполнения, dex.pool_enabled = да и заданы все адреса, без которых пул нельзя проверить, слушать, сверить с
//! квотером и исполнить; иначе — «DEX X не подключена: причина».
//! Кто вызывает: mod.rs (dex_families), pools/* (виды и разбор), directory (сопоставление), quote/*, exec/*.
//! От чего зависит: core/types (PoolKind), gateway/contract (DexFamily), ctx.rs.
//! Вид шага исполнителя (решение 122, часть F) — поле executor_kind (pools/exec_kind.rs): с ним вид исполним через
//! ArbExecutor, когда задан exec.executor_address (exec_kind::via_executor); без адреса — сигнал, как раньше. Решение
//! 395: у Uniswap V4 и PancakeSwap Infinity (tradable — свой UR) тоже есть вид шага (UNI_V4 / INF_*, исполнитель v3).
//! Настройки: dex.kind, dex.pool_enabled и адреса dex.* [DEX индексатора]; smart.universal_router, smart.permit2 [bsc_dex].

use crate::core::types::PoolKind;
use crate::settings::scope::Scope;

use super::super::ctx::Ctx;
use super::super::ctx_place::own_dex;
use super::exec_kind::{self, ExecKind};
use super::family_variant::Variant;

/// Протокол вида пула: от него зависит ABI событий и чтение состояния.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Proto {
    /// V2 и его форки: события Sync, getReserves, getPair фабрики.
    V2,
    /// V3 с ABI PancakeSwap: slot0.feeProtocol uint32, Swap из 9 полей.
    PancakeV3,
    /// V3 с ABI Uniswap: slot0.feeProtocol uint8, Swap из 7 полей.
    UniswapV3,
    /// PancakeSwap Infinity CL: геттеры у CLPoolManager.
    InfinityCl,
    /// PancakeSwap Infinity Bin: геттеры у BinPoolManager.
    InfinityBin,
    /// Uniswap V4: состояние через StateView.
    UniswapV4,
    /// KyberSwap Elastic (решение 81): отдельный контракт на пул (как V3), ликвидность L = baseL + reinvestL,
    /// свои геттеры/события/квотер, свой роутер (форк). Исполнения пока нет — только сигнал (решение 121 B2).
    KyberElastic,
    /// DODO PMM — DODOVendingMachine (DVM): односторонний пул, Q0=0, цели решаются adjustedTarget по резервам
    /// (решения 80, 86). Свои геттеры/события/квотер (сам пул — querySellBase/Quote). Чтение — load_dodo.rs.
    DodoDvm,
    /// DODO PMM — DODOPrivatePool (DPP): двусторонний пул, хранит цели/RState; querySellBase/Quote возвращают
    /// 4 значения (receive, mtFee, RState, newTarget) — иной разбор квотера, чем у DVM/DSP.
    DodoDpp,
    /// DODO PMM — DODOStablePool (DSP): двусторонний пул, хранит цели/RState; querySellBase/Quote возвращают
    /// 4 значения, как DPP.
    DodoDsp,
    /// Algebra V1 (THENA Fusion и форки, решения 80, 81): отдельный контракт на пул (как V3), одна ликвидность,
    /// динамическая комиссия (globalState.fee), СЖАТЫЙ битмап тиков tickTable (слово = floor(tick / tickSpacing) >> 8,
    /// решение 463; у Algebra V1.9 Camelot V3 — AlgebraDirFee — битмап сырой). Свои геттеры/события/квотер (QuoterV2
    /// без параметра fee), своя фабрика (poolByPair — один пул на пару). Математика — math/swap_algebra.rs.
    Algebra,
    /// Algebra Integral (THENA V3, DexSwap; решение 188): тот же обмен, события Swap/Mint/Burn и фабрика, что у
    /// Algebra V1.9, но тики связаны списком (prevTick/nextTick, без чтения битмапа), комиссия — fee() плагина при
    /// DYNAMIC_FEE и добавка плагина (SwapFee). Состояние — PoolState::Algebra, математика — math/swap_algebra.rs.
    /// Чтение — load_algebra_integral.rs, события — events_algebra_integral.rs. Квотер — только виртуальный
    /// (dex.quoter_virtual = да, решение 186): официальный квотер Integral в код не подключён.
    AlgebraIntegral,
    /// Решение 444: Algebra V1.9 с направленными комиссиями (Camelot V3; cryptoalgebra/AlgebraV1.9, ветка
    /// direction-based-fee, a45417ed): всё как у Algebra (обмен, сырой битмап тиков, Swap/Mint/Burn, фабрика poolByPair /
    /// Pool, swap + algebraSwapCallback), но globalState несёт две комиссии (feeZto, feeOtz — по стороне обмена,
    /// AlgebraPool.sol:739) и событие Fee(feeZto, feeOtz). Состояние — PoolState::Algebra с fee_otz; чтение —
    /// load_algebra_dir.rs, события — events_algebra_dir.rs. Квотер — только виртуальный (dex.quoter_virtual = да):
    /// какой квотер развёрнут у DEX (Quoter или QuoterV2 ветки) — не сверено, решение 444, вопрос.
    AlgebraDirFee,
    /// Solidly (ve(3,3)-форки: Topaz v2, THENA V1; решения 80, 85): пул с резервами и флагом `stable` (volatile `x·y=k`
    /// → math/v2.rs; stable `x³y+xy³=k` → math/swap_solidly.rs). Свой Pair-интерфейс (Sync с резервами uint256,
    /// getAmountOut и metadata в самом пуле) — не обычный V2-форк. Чтение — load_solidly.rs, события — events_solidly.rs,
    /// проверка — verify_solidly.rs (фабрика getPool), квотер — сам пул (getAmountOut). Вид ядра — Solidly.
    Solidly,
    /// Slipstream (Topaz v3 на BSC; решения 80, 85): построчный форк Uniswap V3 core (концентрированная ликвидность),
    /// интерфейс пула = Uni V3 (slot0 БЕЗ feeProtocol, liquidity, ticks, tickBitmap, fee, tickSpacing). Математика свопа
    /// и карта тиков — те же, что у V3 (PoolState::V3, math/swap_cl.rs). Отличия от нашего V3: ключ пула фабрики — int24
    /// tickSpacing (getPool(a, b, int24), PoolCreated(…, int24 tickSpacing, pool)); тики — прямыми геттерами пула
    /// (load_slipstream.rs, НЕ TickLens); квотер QuoterV2 — по tickSpacing. Комиссия — с fee() каждого пула (не из
    /// таблицы дефолтов). Вид ядра — V3. Чтение — load_slipstream.rs, события — events_v3.rs (Swap/Mint/Burn как у Uni
    /// V3), проверка — verify.rs (фабрика getPool по int24), квотер — quoter_calls.rs (ISlipstreamQuoterV2).
    Slipstream,
    /// Solidly V1 — THENA V1 (решение 122 F): исходный Solidly-Pair (ThenafiBNB/THENA-Contracts, HEAD f5bc742,
    /// contracts/Pair.sol, PairFactoryUpgradeable.sol). Пул — тот же интерфейс, что у Solidly (metadata, Sync(uint256,
    /// uint256), getAmountOut), но фабрика другая: реестр `getPair(a, b, bool stable)`, комиссия `getFee(bool stable)`
    /// (без адреса пула), событие `PairCreated(token0, token1, bool stable, pair, uint)` (stable НЕ индексирован), и
    /// своя стабильная математика (_f другой формы, _get_y без отката — math/solidly_v1_math.rs). Вид ядра — Solidly.
    SolidlyV1,
    /// StableSwap шаблона Curve 2020 (Vyper 0.2, без A_PRECISION) — Ellipsis StableSwap (решение 122 F;
    /// ellipsis-finance/ellipsis 6bdf778, contracts/StableSwap.vy): N монет в пуле, coins(k)/balances(k)/A()/fee(),
    /// квотер — сам пул get_dy(i, j, dx). Фабрики с реестром нет — пул подтверждает список dex.stable_pools.
    /// Чтение — load_curve.rs, события — events_curve.rs, проверка — verify_curve.rs, математика —
    /// math/stableswap_v1.rs. Вид ядра — StableSwap.
    CurveStable,
    /// PancakeSwap StableSwap (решение 122 F, 3): Solidity-порт шаблона Curve 2020 без A_PRECISION
    /// (pancake-smart-contracts cb079908, projects/stable-swap/contracts/PancakeStableSwapTwoPool.sol; копия —
    /// scratchpad/sources/pancake-stable). Геттеры coins/balances/A/fee — как у Ellipsis (те же селекторы), индексы
    /// обмена и квотера — uint256 (`exchange(uint256,uint256,uint256,uint256)`, `get_dy(uint256,uint256,uint256)`),
    /// событие TokenExchange с индексами uint256. Пул подтверждает реестр фабрики (getPairInfo / getThreePoolPairInfo)
    /// или список dex.stable_pools (пулы первого поколения). Математика — math/stableswap_v1.rs (curve2020).
    PancakeStable,
    /// Lista StableSwap (решение 122 F, 3): форк PancakeStableSwapTwoPool с A_PRECISION = 100 и проверкой цены пула
    /// против оракула после обмена (lista-dao/moolah eb7d73a, src/dex/StableSwapPool.sol). Пул из двух монет, индексы
    /// uint256, пауза (whenNotPaused). Пул подтверждает реестр фабрики getPairInfos(a, b). Цена оракула меняется без
    /// событий пула — состояние перечитывается на каждом новом блоке. Математика — stableswap_v1 (A_PRECISION) +
    /// math/stable_guard.rs.
    ListaStable,
    /// Saddle StableSwap — Nerve (решение 122 F, 3; nerve-finance/contracts bda7578, Swap.sol + SwapUtils.sol):
    /// getToken(uint8)/getTokenBalance(uint8)/getAPrecise()/swapStorage(), квотер calculateSwap(uint8,uint8,uint256),
    /// обмен swap(uint8,uint8,uint256,uint256,uint256 deadline). Пул подтверждает список dex.stable_pools.
    /// Математика — stableswap_v1 (A_PRECISION, 256 раундов, откат при несходимости).
    SaddleStable,
    /// WooFi WooPPV2 (решение 122 F, 3): один общий контракт на все базовые монеты против одной котируемой, цена — от
    /// оракула (без событий оракула — состояние читается на каждом новом блоке). Ребро графа — PoolRef::Member
    /// (контракт, базовая монета). Своей математики нет до проверенного исходника развёрнутой реализации (прокси):
    /// выход — лесенка ответов квотера контракта query(from, to, amount) (dex.pmm_ladder_usd). Вид ядра — OraclePmm.
    WooPP,
    /// ElfomoFi on Base: oracle pricing behind a proxy and one router for many token pairs.
    Elfomo,
    /// LunarBase native ETH/USDC. Исполнение WETH требует отдельной атомарной обёртки.
    Lunar,
    /// BaiBai на Base: книга с коротким сроком жизни и индивидуальная комиссия участника.
    BaiBai,
    /// Maverick V2 (решение 122 F; maverickprotocol/v2-common d91e871, IMaverickV2Pool.sol): тики с резервами tokenA и
    /// tokenB, комиссия по направлению fee(bool), бины разных видов. Расчёт обмена — порт команды Maverick
    /// (math/maverick_swap.rs; Solidity-исходник закрыт), сверка — MaverickV2Quoter.calculateSwap (решение 73). Фабрика —
    /// isFactoryPool / lookup / PoolCreated. Чтение — load_maverick.rs, события — events_maverick.rs, проверка —
    /// verify_maverick.rs. Вид ядра — TickBin.
    Maverick,
    /// PancakeSwap Infinity Stable (решение «122 (Infinity Stable)»): пул StableSwap-NG живёт в КРЮЧКЕ пула Infinity CL
    /// (CLStableSwapHook, verified-код BscScan 0x86c3DC08…, Vyper 0.3.10): один крючок — один пул из двух монет;
    /// балансы, A, комиссия и курсы монет — у крючка, токены — в Vault Infinity. Ссылка на пул — адрес крючка
    /// (PoolRef::Contract: у него состояние, события и квотер get_dy), ключ PoolKey (hooks = крючок) — для обмена через
    /// Universal Router (CL_SWAP_EXACT_IN_SINGLE: beforeSwap крючка забирает весь вход, CL-пул меняет 0). Пул
    /// подтверждает менеджер (poolIdToPoolKey) и список крючков dex.stable_hooks. Курсы монет могут идти от внешних
    /// оракулов без событий — перечитывание на каждом блоке. Математика — math/stableswap_ng.rs. Вид ядра — StableSwap.
    InfinityStable,
    /// Curve StableSwap-NG (решение 275; curve-bsc): plain-пул фабрики NG (curvefi/stableswap-ng 3332cd4,
    /// CurveStableSwapNG.vy v7.0.0) — отдельный контракт, 2..8 монет, A_PRECISION = 100, динамическая комиссия offpeg,
    /// курсы монет stored_rates() (у оракульных / ERC4626 — внешний вызов на блоке; rebasing — balanceOf). Математика
    /// — math/stableswap_ng.rs (та же, что у Infinity Stable: формулы обмена совпадают с NG v7.0.0). События NG
    /// (массивы uint256[] — topic0 иные, чем у Curve 2020) — events_curve_ng.rs; перечитывание на каждом блоке; пул
    /// подтверждает реестр фабрики (get_coins / is_meta, verify_curve_ng.rs); квотер — сам пул get_dy(int128, …);
    /// исполнение — шаг CURVE исполнителя (exchange(int128, int128, dx, 0) — селектор есть у пула). Вид ядра — StableSwap.
    CurveNg,
    /// Nomiswap Stable (решение 284; nomiswap_stable): пара NomiswapStablePair (verified Sourcify 56/0x1e40…60f1) — две
    /// монеты, стейблсвап с A·100 (рампа по времени блока), комиссия swapFee над 100000 СО ВХОДА; фабрика, поля пары,
    /// события Sync/Swap/Mint/Burn и обмен swap(out0, out1, to, data) — как у V2 (проверка и поиск — путём V2:
    /// getPair / PairCreated). Квотер — сама пара getAmountOut(address, uint256); математика — math/stableswap_nomi.rs
    /// (StableData.input_fee). swapFee и A меняются без события — перечитывание каждым блоком. Исполнение — шаг
    /// NOMI_STABLE исполнителя v2. Вид ядра — StableSwap.
    NomiStable,
    /// Metric OMM (решение 285; metric_omm, DEX metric-bsc): пул MetricOmmPool фабрики Metric — цена от провайдера
    /// (меняется каждым блоком), бины ликвидности, расширения (хуки). Исходник BSC не опубликован — своей математики нет:
    /// котировка и глубина — лесенка ответов САМОГО пула через виртуальный квотер (quoteMetric, решение 186;
    /// dex.pmm_ladder_usd), как WooFi. Порядок токенов пула не по адресу (token0 из getImmutables — OraclePmmData.
    /// pool_token0). Проверка — реестр фабрики isPool + getImmutables (verify_metric.rs); новые пулы — из справочника;
    /// перечитывание каждым блоком. Исполнение — шаг METRIC исполнителя v2 (swap + metricOmmSwapCallback). Вид ядра —
    /// OraclePmm.
    MetricOmm,
    /// iZiSwap (решение 286; iziswap, DEX iziswap-bsc): пул iZiSwapPool (izumiFinance/iZiSwap-core) — дискретные точки
    /// с ликвидностью и лимитными ордерами, обмен swapX2Y / swapY2X (tokenX < tokenY) + обратные вызовы
    /// swapX2YCallback / swapY2XCallback. Своей математики точек нет — котировка и глубина по лесенке ответов САМОГО
    /// пула через виртуальный квотер (quoteIzi; dex.pmm_ladder_usd), как Metric. Проверка — поля пула + реестр фабрики
    /// pool(x, y, uint24 fee) (verify_izi.rs); новые пулы — из справочника; перечитывание каждым блоком и на события.
    /// Исполнение — шаг IZI исполнителя v2. Вид ядра — OraclePmm.
    IziSwap,
    /// Biswap V3 (решение 286; biswap_v3, DEX biswap-v3 и biswap-v3-1): форк iZiSwap (BiswapPoolV3, Sourcify) — те же
    /// swapX2Y / swapY2X и обратные вызовы (факт цепи), но реестр фабрики pool(x, y, uint16 fee), иные события (Swap со
    /// структурами, ChangeFeeVote — голосуемая комиссия) и возврат обмена из трёх значений. Всё прочее — как IziSwap.
    BiswapV3,
    /// KyberSwap Classic DMM (решение 287; kyber_dmm, DEX kyberswap_classic_bsc): пул DMMPool (dmm-smart-contracts@e557b57,
    /// фабрика BSC — Sourcify match) — пара V2 с усилением amp: x·y=k по виртуальным резервам, выход ограничен реальным
    /// резервом, комиссия динамическая (EMA объёма, своя на каждом блоке — getTradeInfo). У пары токенов может быть
    /// несколько пулов (getPools). Поля пула и обмен swap(out0, out1, to, data) — как у V2. Математика — math/dmm.rs,
    /// квотер — DMMRouter02.getAmountsOut (dex.quoter). Перечитывание каждым блоком. Исполнение — шаг KYBER_DMM
    /// исполнителя v2. Вид ядра — AmpV2.
    KyberDmm,
    /// SmarDex (решение 288; smardex, DEX smardex-bsc): пара SmardexPair (фабрика BSC — Sourcify match = официальный
    /// репозиторий SmarDex-Ecosystem/dex-smart-contracts, deployments/bsc) — x·y=k по фиктивным резервам, обмен делится на
    /// два шага по средней цене, которая пересчитывается к block.timestamp обмена; комиссии — в паре (getPairFees). Фабрика
    /// getPair / PairCreated и поля пары — как у V2 (проверка и поиск — путём V2). Обмен swap(to, zeroForOne, сумма, data)
    /// и обратный вызов smardexSwapCallback. Математика — math/smardex.rs, квотер — только виртуальный (quoteSmardex). Перечитывание
    /// каждым блоком. Исполнение — шаг SMARDEX исполнителя v2. Вид ядра — Fictive.
    Smardex,
    /// Balancer V2 (решение 289; balancer_v2, DEX acsi-finance — ACSI Finance BSC): хранилище Vault (Sourcify exact_match =
    /// balancer-deployments 20210418-vault) и пулы его фабрик — весовой WeightedPool (dex.factory) и StablePool
    /// (dex.balancer_stable_factory), тексты = build-info Balancer. Токены всех пулов — в хранилище; обмен —
    /// Vault.swap по id пула; формула — пула (math/balancer_math.rs, balancer_stable.rs). Проверка — getVault/getPoolId
    /// пула, getPool(id) хранилища, isPoolFromFactory, getPoolTokens (verify_balancer.rs); новые пулы — из справочника;
    /// перечитывание каждым блоком (балансы меняют события хранилища, не пула). Квотер — Vault.queryBatchSwap
    /// (dex.vault). Исполнение — шаг BALANCER исполнителя v2. Вид ядра — VaultPool.
    BalancerV2,
    /// Liquidity Book V2.1 (решение 290; lb_v21, DEX traderjoe-v2-1-bsc — LFJ / Trader Joe на BSC): пул LBPair — свой
    /// контракт (клон реализации фабрики), бины с резервами X и Y, порядок X/Y — создателя пула; комиссия = база +
    /// изменчивая часть (растёт с пройденными бинами, затухает по времени блока). Фабрика LBFactory (dex.factory) —
    /// реестр getLBPairInformation (Sourcify exact_match = lfj-gg/joe-v2@v2.1.0). Проверка — getFactory / getTokenX /
    /// getTokenY / getBinStep пула и реестр фабрики (verify_lb.rs); новые пулы — из справочника. Математика — порт
    /// getSwapOut (math/lb_swap.rs, lb_fee.rs); перечитывание каждым блоком (время блока). Квотер сверки — сам пул
    /// getSwapOut. Исполнение — шаг LB исполнителя v2 (вход переводом в пул, swap(swapForY, to)). Вид ядра — LbBin.
    LbV21,
    /// Fluid DEX T1 (решение 440; fluid_t1, DEX fluid-arbitrum): пул FluidDexT1 (Instadapp/fluid-contracts-public
    /// @404d73f, poolT1) — ликвидность в слое Liquidity как смарт-залог и/или смарт-долг, x·y=k по мнимым резервам частей
    /// вокруг центральной цены, обмен делится между частями. Фабрика FluidDexFactory (dex.factory) — isDex; состояние —
    /// официальный резолвер FluidDexReservesResolver (dex.quoter: getPoolReservesAdjusted, сверка — estimateSwapIn) и
    /// слова пула; новые пулы — из справочника. Математика — порт _swapIn (math/fluid_swap.rs, fluid_check.rs);
    /// перечитывание каждым блоком. Нативная монета пула (0xEeee…) — в паре обёртка dex.native_wrapped. Исполнение — шаг
    /// FLUID исполнителя v4 (approve пулу + swapIn). Вид ядра — SmartColDebt.
    FluidT1,
    /// Shell Protocol Proteus (решение 486; shell_proteus, DEX Shell на Arbitrum): пул LiquidityPoolProxy с внешней
    /// реализацией Proteus (кусочная гипербола по срезам, полезность u неизменна при обмене); токены всех пулов — в
    /// учётном контракте Ocean (ERC-1155, dex.vault), обмен — через него (wrap → computeOutputAmount пула → unwrap).
    /// Фабрики с реестром нет — пулы из списка dex.stable_pools (адреса — docs.shellprotocol.io); формула полезности —
    /// по спискам сверенных реализаций dex.proteus_utility_v1 / _v2. Математика — порт Proteus.sol и ABDKMath64x64
    /// (math/proteus_*.rs); перечитывание каждым блоком и на события пула. Исполнение — шаг SHELL исполнителя v5.
    /// Вид ядра — Proteus.
    ShellProteus,
    /// Balancer V3 (решение 441, T433-F2; balancer_v3, DEX balancer-v3-arbitrum): хранилище Vault V3 (одно на все пулы,
    /// id пула — его адрес) и пулы фабрик Balancer — весовые (20241205, 20260115) и стейблсвап (20241205, 20250324,
    /// 20260116); выпуск фабрики задаёт правило onSwap (списки dex.bv3_*_factories). Токены — в хранилище; хранилище
    /// пересчитывает суммы курсом токена (поставщик курса) и снимает комиссию; пулы с крючком обмена не берутся.
    /// Проверка — verify_balancer_v3.rs, чтение — load_balancer_v3*.rs (каждым блоком: балансы и курсы меняются без
    /// событий пула), события — events_balancer_v3.rs, математика — math/balancer_v3*.rs, квотер — Router V3
    /// querySwapSingleTokenExactIn (dex.quoter). Исполнение — шаг BALANCER_V3 исполнителя (unlock → swap → settle →
    /// sendTo). Вид ядра — VaultPool (состояние с частью V3).
    BalancerV3,
    /// Curve Twocrypto-NG (решение 442; curve_twocrypto_ng): crypto-пул из двух монет разной цены фабрики Twocrypto-NG
    /// (curvefi/twocrypto-ng 89c8dfb, тег prod-v2.1.0, CurveTwocryptoOptimized.vy) — инвариант с A и gamma, внутренняя
    /// цена price_scale, динамическая комиссия mid_fee → out_fee. Математика — math/crypto_y2.rs, crypto_swap.rs (своя
    /// математика пула — внешний контракт MATH(): пул берётся, только если его адрес в dex.curve_crypto_math, а
    /// version() — в dex.curve_crypto_versions). Чтение — load_crypto.rs, события — events_crypto.rs, проверка — реестр
    /// фабрики get_coins (verify_crypto.rs), поиск — find_pool_for_coins (factory_ask.rs); квотер — сам пул
    /// get_dy(uint256, …); исполнение — шаг STABLE_U256 исполнителя (exchange(uint256, uint256, dx, 0)). Вид ядра — Crypto.
    CurveTwocrypto,
    /// Curve Tricrypto-NG (решение 442; curve_tricrypto_ng): crypto-пул из трёх монет фабрики Tricrypto-NG
    /// (curvefi/tricrypto-ng ecaa816, CurveTricryptoOptimizedWETH.vy / CurveTricryptoOptimized.vy) — всё как у
    /// CurveTwocrypto, математика — math/crypto_y3.rs, комиссия — reduction_coefficient. Многомонетный (решение 295).
    CurveTricrypto,
    /// Curve crypto v1 фабрики (решение 453; curve_crypto_v1): двухмонетный crypto-пул первого поколения
    /// (curvefi/curve-factory-crypto 7ed1011, CurveCryptoSwap2ETH.vy; пулы — прокси EIP-1167 на шаблон фабрики) —
    /// математика внутри пула (math/crypto_v1_2.rs), без precisions()/version()/MATH(): точность монет — по decimals,
    /// шаблон — по коду прокси (dex.curve_crypto_v1_templates). Чтение — load_crypto_v1.rs, события —
    /// abi_crypto_v1.rs, проверка — реестр фабрики get_coins + код прокси (verify_crypto_v1.rs), поиск —
    /// find_pool_for_coins. Квотер — get_dy пула; исполнение — шаг STABLE_U256. Вид ядра — Crypto.
    CurveCryptoV1,
    /// Curve tricrypto 2021 (решение 453; curve_tricrypto_v1): трёхмонетный crypto-пул первого поколения
    /// (curvefi/curve-crypto-contract a0f8401, CryptoSwap.vy + CurveCryptoMath3.vy Vyper 0.2.15) — пул без фабрики,
    /// подтверждает список dex.stable_pools (журнал развёртывания curvefi); математика — math/crypto_v1_3.rs.
    /// Многомонетный (решение 295). Квотер — get_dy пула; исполнение — шаг STABLE_U256. Вид ядра — Crypto.
    CurveTricryptoV1,
    /// Camelot V2 (решение 443, T433-F4; camelot_v2, DEX camelot сети Arbitrum): пара CamelotPair — резервы uint112,
    /// НАПРАВЛЕННАЯ комиссия (token0FeePercent / token1FeePercent над 100000, меняется событием FeePercentUpdated) и
    /// флаг stableSwap (volatile — x·y=k, stable — x³y+xy³; меняется событием SetStableSwap). Фабрика getPair /
    /// PairCreated, поля пары factory/token0/token1, Sync / Mint, обмен swap(out0, out1, to, data) — как у V2 (проверка
    /// и поиск — путём V2). Состояние — PoolState::Solidly с fee1 (load_camelot.rs), события — events_camelot.rs,
    /// математика — math/camelot_v2.rs (арифметика Solidity 0.5), квотер — сама пара getAmountOut(uint256, address)
    /// (селектор тот же, что у Solidly). Исполнение — шаг SOLIDLY исполнителя. Вид ядра — Solidly.
    CamelotV2,
    /// Решение 470 (T451-02; aero_basic, Aero «Basic» сети Arbitrum — Dromos-Labs/metadex-public 1.1.0): пул
    /// VolatilePool / StablePool — кривые Solidly, но metadata() из 6 полей (без stable; кривая — POOL_TYPE()),
    /// фабрика на одну кривую (getPool(a, b), PoolCreated без stable), комиссия — getBaseFee фабрики, MEV-налог модуля
    /// (aero_mev.rs). Состояние — PoolState::Solidly (load_aero_basic.rs), события — events_solidly.rs (Sync тот же),
    /// проверка и поиск — путём V2 (factory/token0/token1, getPool), квотер — сам пул getAmountOut. Исполнение — шаг
    /// SOLIDLY исполнителя. Вид ядра — Solidly.
    AeroBasic,
    /// Liquidity Book v2.0 (решение 445; lb_v20, DEX traderjoe-v2-arbitrum — LFJ V2 на Arbitrum): тот же пул из бинов, но
    /// ABI v2.0 (tokenX / factory / getReservesAndId / feeParameters / findFirstNonEmptyBinId, события на бин) и правила
    /// v2.0 (комиссия вне бина, пределы uint24/uint112, без MAX_FEE). Фабрика LBFactory v2.0 (dex.factory) — реестр
    /// getLBPairInformation; квотер сверки — LBRouter v2.0 getSwapOut (dex.quoter). Исходник — lfj-gg/joe-v2@v2.0.0.
    /// Математика — math/lb_swap.rs с ветками v20 (lb20_fee.rs, lb20_swap.rs); перечитывание каждым блоком. Исполнение —
    /// шаг LB20 исполнителя (вход переводом в пул, swap(tokenX() == вход, to)). Вид ядра — LbBin (признак v20).
    LbV20,
    /// Liquidity Book V2.2 (решение 446; lb_v22, DEX traderjoe-v2-2-arbitrum — LFJ V2.2 на Arbitrum One): тот же пул,
    /// что у V2.1 (геттеры, события, swap, getSwapOut — lfj-gg/joe-v2@v2.2.0 1297c38), отличия: крючки пула (вызов
    /// beforeSwap / afterSwap в swap — pools/lb_hooks.rs: крючок обмена допускается только из dex.lb_hooks_allowed),
    /// откат обмена при ликвидности бина больше MAX_LIQUIDITY_PER_BIN (math/lb_cap.rs), другая раскладка хранилища
    /// (_parameters — слот 3, _reserves — 4: защита от повторного входа и Initializable — в своих слотах ERC-7201),
    /// реализация пула implementation() — из dex.lb_implementations (pools/verify_lb22.rs). Вид ядра — LbBin.
    LbV22,
    /// Ekubo (решение 447; ekubo, DEX ekubo-v3-arbitrum): синглтон-ядро Core (EkuboProtocol/evm-contracts@61d09daa) — все
    /// пулы в одном контракте, ключ (token0, token1, config: расширение, комиссия 0.64, шаг тика или стейблсвап), PoolId =
    /// keccak256 ключа. Ссылка — PoolRef::Singleton (Core, PoolId). Ключ пула — у PoolKeyIndex (verify_ekubo.rs);
    /// состояние, битмап и тики — Core.sload (load_ekubo.rs); события Core — анонимный лог обмена (log0) и PositionUpdated
    /// (events_ekubo.rs); математика — math/ekubo_swap.rs; расширения — нет или MEVCapture (доплата — math/ekubo_mev.rs).
    /// Сверка — Router.quote (MEVCaptureRouter, dex.quoter). Исполнение — шаг EKUBO исполнителя (lock → swap / forward).
    /// Вид ядра — Ekubo.
    Ekubo,
    /// Решение 484 (wombat, DEX wombat-exchange сети Arbitrum): пул Wombat Exchange (wombat-exchange/v1-core 2ac067b8;
    /// развёрнутые реализации HighCovRatioFeePoolV2 / DynamicPoolV2 — Sourcify, docs/sources/wombat_arbitrum/) —
    /// стейблсвап с коэффициентом покрытия: у каждой монеты свой актив (cash, liability), haircut с выхода, комиссия за
    /// покрытие входа выше startCovRatio; у динамического пула — относительные цены активов. Пул — прокси, реализация
    /// из dex.wombat_implementations / dex.wombat_dynamic_implementations; пулы — список dex.stable_pools (фабрики нет).
    /// Чтение — load_wombat*.rs (каждым блоком: mintFee и цены активов меняются без событий пула), события —
    /// events_wombat.rs, проверка — verify_wombat.rs, математика — math/wombat*.rs, квотер — quotePotentialSwap пула
    /// (quote/quoter_wombat.rs). Исполнение — шаг WOMBAT исполнителя v5. Многомонетный. Вид ядра — StableSwap (часть
    /// StableData.wombat).
    Wombat,
    /// Скважина Basin (решение 485; basin_well, DEX Basin на Arbitrum): Well (BeanstalkFarms/Basin@ecf69233) — пул из
    /// двух токенов без своей комиссии, цену задаёт функция скважины ConstantProduct2 или Stable2 (адреса —
    /// dex.basin_cp2 / dex.basin_stable2, A Stable2 — dex.basin_stable2_lut) от резервов и totalSupply. Реестр — Aquifer
    /// (dex.factory: wellImplementation, событие BoreWell); скважины Beanstalk — прокси ERC-1967 на клон WellUpgradeable
    /// (реализации — dex.basin_well_impls). Состояние — load_basin.rs, события Swap / Shift — events_basin.rs, проверка —
    /// verify_basin.rs, математика — math/basin_well.rs, сверка — getSwapOut самой скважины. Исполнение — шаг BASIN
    /// исполнителя v5 (approve скважине + swapFrom). Вид ядра — BasinWell.
    BasinWell,
}

impl Proto {
    /// Многомонетный вид (решение 295): пул держит 2..8 монет в одном контракте и может дать боту несколько пар —
    /// Curve 2020 (Ellipsis), Curve StableSwap-NG, Balancer V2, Saddle (Nerve), PancakeSwap StableSwap, Curve
    /// Tricrypto-NG (решение 442). Lista (форк
    /// TwoPool), Infinity Stable (N_COINS = 2) и Nomiswap Stable (пара) — не входят. Ошибок нет.
    pub fn is_multi_coin(self) -> bool {
        matches!(
            self,
            Proto::CurveStable
                | Proto::CurveNg
                | Proto::BalancerV2
                | Proto::BalancerV3
                | Proto::SaddleStable
                | Proto::PancakeStable
                | Proto::CurveTricrypto
                | Proto::CurveTricryptoV1
                | Proto::Wombat
        )
    }

    /// Crypto-пул Curve (решения 442, 453): Twocrypto-NG, Tricrypto-NG, crypto v1 фабрики, tricrypto 2021. Ошибок нет.
    pub fn is_curve_crypto(self) -> bool {
        matches!(
            self,
            Proto::CurveTwocrypto
                | Proto::CurveTricrypto
                | Proto::CurveCryptoV1
                | Proto::CurveTricryptoV1
        )
    }

    /// Семейство StableSwap (все шаблоны: Curve 2020 Ellipsis, PancakeSwap, Lista, Saddle, NG, Nomiswap). Ошибок нет.
    pub fn is_stable(self) -> bool {
        matches!(
            self,
            Proto::CurveStable
                | Proto::PancakeStable
                | Proto::ListaStable
                | Proto::SaddleStable
                | Proto::InfinityStable
                | Proto::CurveNg
                | Proto::NomiStable
        )
    }

    /// Семейство DODO (DVM/DPP/DSP) — PMM-пул. Ошибок нет.
    pub fn is_dodo(self) -> bool {
        matches!(self, Proto::DodoDvm | Proto::DodoDpp | Proto::DodoDsp)
    }

    /// Семейство Solidly (ve(3,3)-форки: Topaz v2 — Solidly, THENA V1 — SolidlyV1). Ошибок нет.
    pub fn is_solidly(self) -> bool {
        matches!(self, Proto::Solidly | Proto::SolidlyV1)
    }

    /// Семейство iZiSwap (iZiSwap и форк Biswap V3, решение 286). Ошибок нет.
    pub fn is_izi(self) -> bool {
        matches!(self, Proto::IziSwap | Proto::BiswapV3)
    }
}

/// Вид пула одной DEX. Живёт всю работу процесса (одна запись на пару «id, протокол»). Debug — вручную (ниже): ячейки
/// настроек `event` (ревью 325) в вывод не входят, вывод побайтно прежний (золотые снимки tests/golden_common).
pub struct Family {
    /// id DEX у индексатора — id области «DEX» (dex.pool_enabled, адреса контрактов).
    pub id: &'static str,
    pub kind: PoolKind,
    pub proto: Proto,
    /// Название в журнале и интерфейсе — тот же id.
    pub title: &'static str,
    /// Пул отслеживается и входит в граф маршрутов для поиска/сигналов (решение 80): true у поддержанных DEX и у
    /// V2-форков (общая V2-математика, своя комиссия пары). false — вид только в справочнике (например V3-форки
    /// пока без котировки). Отделено от tradable: форк обогащает сигналы ещё до готовности исполнения (решение 82).
    pub graphable: bool,
    /// Есть путь исполнения: Universal Router своей фабрики (не форк) ИЛИ собственный роутер форка (fork=true, бандлом,
    /// фаза 80.9b). false — вид только в справочнике/сигнале (например V3-форк): авто-сделкой не исполняется.
    pub tradable: bool,
    /// Исполняется через СОБСТВЕННЫЙ роутер форка (classic V2 Router, swapExactTokens…SupportingFeeOnTransferTokens,
    /// ERC-20 approve без Permit2), а НЕ через Universal Router своей фабрики (решения 59, 82, фаза 80.9b). У
    /// поддержанных DEX (PancakeSwap/Uniswap) false — у них Universal Router. Используется сборкой бандла
    /// (exec/bundle.rs, exec/fork_swap.rs) и классификацией формы исполнения (quote/graph_route.rs).
    pub fork: bool,
    /// Вид шага контракта-исполнителя (решение 122, часть F; pools/exec_kind.rs). None — исполнитель пулы этого вида
    /// не вызывает. Исполнимость через исполнитель — exec_kind::via_executor (нужен ещё exec.executor_address).
    pub executor_kind: Option<ExecKind>,
    /// Решение 451: вариант исходника внутри протокола (pools/family_variant.rs; по значению dex.kind). Base — у всех
    /// прежних видов. В Debug выводится только не Base (золотые снимки прежних видов — побайтно прежние).
    pub variant: Variant,
    /// Ревью 325 (Н1): настройки решения 325 этого DEX в ячейках памяти (event_flags.rs) — путь события пула не
    /// читает хранилище на каждое событие. Не часть значения вида: в Debug не входит; вид один на процесс (&'static,
    /// без Clone) — все пользователи видят одну и ту же живую ячейку.
    pub event: super::event_flags::EventFlags,
}

impl Family {
    pub fn direct_module(&self) -> bool {
        ((self.variant.wallet_module() || matches!(self.variant,Variant::Virtuals | Variant::ElasticV5)) && self.tradable)
            || matches!(self.variant, Variant::SolidlyCl | Variant::BaseNativeV3)
            || matches!(self.proto, Proto::Elfomo | Proto::Lunar | Proto::BaiBai)
    }
}

impl std::fmt::Debug for Family {
    /// Что делает: тот же вывод, что дал бы #[derive(Debug)] без поля `event` (те же имена и порядок полей).
    /// Ошибок нет (кроме ошибки форматтера).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("Family");
        d.field("id", &self.id)
            .field("kind", &self.kind)
            .field("proto", &self.proto)
            .field("title", &self.title)
            .field("graphable", &self.graphable)
            .field("tradable", &self.tradable)
            .field("fork", &self.fork)
            .field("executor_kind", &self.executor_kind);
        if self.variant != Variant::Base {
            d.field("variant", &self.variant);
        }
        d.finish()
    }
}

/// Вариант dex.kind (список — settings::catalog::dex::DEX_KINDS, один на код и каталог) → математика, протокол,
/// graphable (в графе для поиска/сигналов), tradable (есть путь исполнения) и fork (исполняется своим роутером форка,
/// а не Universal Router). Поддержанные DEX (PancakeSwap/Uniswap): graphable+tradable, не форк (Universal Router своей
/// фабрики; решения 31, 38, 59). V2-форк: graphable+tradable+fork (исполняется своим роутером бандлом, фаза 80.9b).
/// V3-форки пока только справочник (graphable=false). None — неизвестен или «не поддерживается». Ошибок нет.
pub fn spec(kind: &str) -> Option<(PoolKind, Proto, bool, bool, bool)> {
    //                                    graphable, tradable, fork
    Some(match kind {
        "pancakeswap_v2" | "uniswap_v2" | "v2_omni_universal" | "ring_few_v2" => {
            (PoolKind::V2, Proto::V2, true, true, false)
        }
        "pancakeswap_v3" => (PoolKind::V3, Proto::PancakeV3, true, true, false),
        "uniswap_v3" => (PoolKind::V3, Proto::UniswapV3, true, true, false),
        "pancakeswap_infinity_cl" => (PoolKind::Cl, Proto::InfinityCl, true, true, false),
        "pancakeswap_infinity_bin" => (PoolKind::Bin, Proto::InfinityBin, true, true, false),
        "launch_doppler" | "launch_index" | "launch_o1" | "deli_v4_constant" | "deli_v4_cl" | "uniswap_v4" => (PoolKind::Cl, Proto::UniswapV4, true, true, false),
        // V2-форк (Biswap и пр.): в графе и ИСПОЛНИМ своим роутером бандлом (fork=true, tradable=true с 80.9b-ii).
        "antfarm" | "temple" | "temple_v5" | "carbon_v1" | "mint_club_v2" | "virtuals_bonding" | "virtuals_unicorn" => (PoolKind::V2, Proto::V2, true, false, true),
        "candyswap_module" | "virtuals_bonding_module" | "virtuals_unicorn_module" | "memebox_v5" | "bunnyswap_module" => (PoolKind::V2, Proto::V2, true, true, true),
        "bunnyswap" | "memebox" | "ethervista_native" | "ethervista_v5" | "oasisswap_v2_router" | "oasisswap_v2_v5" | "fraxswap_twamm" | "fraxswap_twamm_v5" | "fraxswap_twamm_router" => {
            (PoolKind::V2, Proto::V2, true, false, true)
        }
        "v2_fork" | "gemswap_v2" | "v2_diamond_router" | "v2_diamond_create" | "v2_deltaswap_router" | "v2_fork_router_create2" | "v2_fork_dual_create2" | "v2_fork_dual_create2_live_fee" | "v2_whiteswap" => {
            (PoolKind::V2, Proto::V2, true, true, true)
        }
        // V2-форк без известного роутера (решение 280): в графе и сигналах как v2_fork (V2-математика, своя комиссия
        // dex.v2_fee_pct), но tradable=false — своего роутера нет; исполнение — шаг V2 контракта-исполнителя (обмен
        // прямо в паре, exec_kind::via_executor). fork=true — как у v3_fork: не в классический прямой путь и без UR.
        // Сверка расчёта — обмен через код исполнителя в симуляции (quote/v2_pair_quote.rs), а не getAmountsOut роутера.
        "v2_fork_executor" | "v2_fork_executor_create2" | "v2_fork_executor_create2_live_fee" | "candyswap_v2" => {
            (PoolKind::V2, Proto::V2, true, false, true)
        }
        // V3-форк (решение 122 F): в графе и сигналах — чтение, события, проверка и математика как у V3 своего ABI
        // (Uniswap / PancakeSwap: фабрика getPool, TickLens, QuoterV2 — настройки этой DEX). tradable=false: своего
        // пути через Universal Router нет; исполнение — шаг V3 контракта-исполнителя (exec_kind::via_executor).
        // fork=true оставлен: пул не попадает в классический прямой путь (quote/classic_direct.rs) и не требует UR.
        // Решение 483: Ramses CL — V3-форк ABI Uniswap (вариант RamsesCl: currentFee, ticks из 10 полей, FeeAdjustment,
        // ramsesV2SwapCallback; pools/ramses.rs, load_ramses.rs).
        "v3_fork" | "v3_fork_ramses" | "v3_fork_uniswap_lm" => {
            (PoolKind::V3, Proto::UniswapV3, true, false, true)
        }
        "hello_v3" => (PoolKind::V3,Proto::PancakeV3,true,false,true),
        "registered_uni_v3" | "registered_v3" => (PoolKind::V3,Proto::UniswapV3,true,false,true),
        "umbrae_dlmm" => (PoolKind::OraclePmm,Proto::V2,true,false,true),
        "umbrae_dlmm_module" => (PoolKind::OraclePmm,Proto::V2,true,true,true),
        "umbrae_damm" => (PoolKind::V2,Proto::V2,true,false,true),
        "umbrae_damm_module" => (PoolKind::V2,Proto::V2,true,true,true),
        "registered_v3_module" => (PoolKind::V3,Proto::UniswapV3,true,true,true),
        "base_native_v3" => (PoolKind::V3, Proto::PancakeV3, true, false, false),
        "base_native_v3_module" => (PoolKind::V3, Proto::PancakeV3, true, true, true),
        "v3_fork_pancake" => (PoolKind::V3, Proto::PancakeV3, true, false, true),
        // KyberSwap Elastic (решение 81): в графе, котируется своей математикой (math/swap_elastic). Решение 121 B2:
        // tradable=false — своего пути исполнения нет: в exec/ нет ветки Proto::KyberElastic, а нога форка собиралась
        // вызовом classic V2 Router (exec/fork_swap.rs), которого у роутера Elastic (свой контракт со своим
        // интерфейсом) нет — своп откатился бы. Пока нет своего пути через роутер Elastic — только сигнал (guard
        // executable в quote/graph_route.rs). fork=true оставлен: так пул не попадает в классический прямой путь
        // (quote/mod.rs, фильтр !fork) и не требует Universal Router/Permit2 (required). Чтение состояния/события/
        // квотер — свои (не как Infinity/V4). Вид ядра — Cl, ссылка на пул — отдельный контракт (как V3).
        // Решение 451: kyber_elastic_v1 — тот же вид для развёртываний Elastic v1 (Arbitrum), отличие — правило шага
        // SwapMath (pools/family_variant.rs Variant::ElasticV1).
        "kyber_elastic_module" => (PoolKind::Cl, Proto::KyberElastic, true, true, true),
        "kyber_elastic_v5" => (PoolKind::Cl, Proto::KyberElastic, true, false, false),
        "kyber_elastic" | "kyber_elastic_v1" => {
            (PoolKind::Cl, Proto::KyberElastic, true, false, true)
        }
        // DODO PMM (решения 80, 86b): в графе (своя математика math/swap_dodo), НО tradable=false — исполнения
        // пока нет (роутер DODO — фаза 86c). graphable=true, tradable=false, fork=false (ни UR, ни свой
        // роутер-форк): как v3_fork до появления пути исполнения, но в графе (экзотическая математика берётся).
        // Квотер — сам пул (querySellBase/Quote), отдельного контракта-квотера нет.
        "dodo_dvm" => (PoolKind::Pmm, Proto::DodoDvm, true, false, false),
        "dodo_dpp" => (PoolKind::Pmm, Proto::DodoDpp, true, false, false),
        "dodo_dsp" => (PoolKind::Pmm, Proto::DodoDsp, true, false, false),
        // Algebra (THENA Fusion, решения 80, 81, фаза B): в графе (своя математика math/swap_algebra). tradable=false —
        // только про Universal Router / бандл (UR своей фабрики и свой роутер-форк не подключены); исполнение —
        // шаг ALGEBRA контракта-исполнителя ArbExecutor (exec_kind.rs, с v1; решение 122 X): маршрут, где каждый шаг —
        // вид исполнителя, идёт через него раньше проверки tradable (quote/graph_exec.rs, exec/cycle_gate.rs). Вид ядра — Cl.
        "thena_fusion" | "algebra_v19_single" => (PoolKind::Cl, Proto::Algebra, true, false, false),
        // Algebra Integral (THENA V3, DexSwap; решение 188): как thena_fusion — в графе (математика swap_algebra),
        // исполнение — шаг ALGEBRA контракта-исполнителя (тот же swap + algebraSwapCallback, exec_kind.rs).
        "algebra_integral"
        | "algebra_integral_treble"
        | "algebra_integral_base_plugin_v1"
        | "algebra_integral_hydrex"
        | "algebra_integral_quickswap"
        | "algebra_integral_nami" | "algebra_integral_supernova" => (PoolKind::Cl, Proto::AlgebraIntegral, true, false, false),
        // Решение 444: Algebra V1.9 с направленными комиссиями (Camelot V3) — как thena_fusion: в графе (математика
        // swap_algebra, комиссия стороны), исполнение — шаг ALGEBRA исполнителя (exec_kind.rs).
        "algebra_dir_fee" => (PoolKind::Cl, Proto::AlgebraDirFee, true, false, false),
        // Solidly ve(3,3)-форки (Topaz v2, THENA V1; решения 80, 85): в графе (своя математика — volatile math/v2,
        // stable math/swap_solidly), НО tradable=false — исполнения пока нет (роутер Solidly — следующая фаза).
        // graphable=true, tradable=false, fork=false (ни UR своей фабрики, ни свой роутер-форк пока не подключены): как
        // DODO 86b / THENA Algebra — в графе и для сигналов, но guard executable (exec/chain.rs) в авто-сделку не пускает.
        "solidly_v2" => (PoolKind::Solidly, Proto::Solidly, true, false, false),
        // Solidly V1 — THENA V1 (решение 122 F): тот же пул, своя фабрика и своя стабильная математика (Proto::SolidlyV1).
        // В графе и сигналах; исполнение — шаг SOLIDLY контракта-исполнителя (exec_kind::via_executor).
        // Решение 483: Ramses Legacy V2 — тот же вид (вариант RamsesLegacy: getAmountOut − 1, Sync uint112, PairCreated без
        // stable; pools/ramses.rs).
        "solidly_v1" | "solidly_v1_generator" | "solidly_v2_proxy" | "solidly_v1_ramses" | "leetswap_v2" => {
            (PoolKind::Solidly, Proto::SolidlyV1, true, false, false)
        }
        // StableSwap (Ellipsis; решение 122 F): в графе и сигналах; исполнение — шаг CURVE контракта-исполнителя.
        "curve_stableswap" => (PoolKind::StableSwap, Proto::CurveStable, true, false, false),
        // Решение 472: Curve StableSwap 2021 (2pool Arbitrum, plain-пулы старой фабрики) — тот же протокол, вариант
        // CurvePlain (family_variant.rs): A_precise(), строгая сходимость, D_P пары; шаг CURVE исполнителя.
        "curve_stableswap_plain" => (PoolKind::StableSwap, Proto::CurveStable, true, false, false),
        // Решение 472 (доделка): метапулы фабрики Curve Sidechain/L2 — вариант CurveMeta, шаг CURVE исполнителя.
        "curve_stableswap_meta" => (PoolKind::StableSwap, Proto::CurveStable, true, false, false),
        // Решение 481: пул Curve с курсом оракула и нативным ETH (ETH/wstETH Arbitrum) — вариант CurveOracle, шаг
        // CURVE_ETH исполнителя v5.
        "curve_stableswap_oracle" => (PoolKind::StableSwap, Proto::CurveStable, true, false, false),
        // Curve StableSwap-NG (решение 275): в графе и сигналах; исполнение — шаг CURVE контракта-исполнителя.
        "curve_stableswap_ng" => (PoolKind::StableSwap, Proto::CurveNg, true, false, false),
        // Nomiswap Stable (решение 284): в графе и сигналах; исполнение — шаг NOMI_STABLE контракта-исполнителя (только v2).
        "nomiswap_stable" => (PoolKind::StableSwap, Proto::NomiStable, true, false, false),
        // Metric OMM (решение 285): в графе и сигналах по лесенке ответов пула; исполнение — шаг METRIC (только v2).
        "metric_omm" => (PoolKind::OraclePmm, Proto::MetricOmm, true, false, false),
        // iZiSwap и Biswap V3 (решение 286): в графе и сигналах по лесенке ответов пула; исполнение — шаг IZI (только v2).
        "iziswap" | "izi_static_modules" => (PoolKind::OraclePmm, Proto::IziSwap, true, false, false),
        "biswap_v3" => (PoolKind::OraclePmm, Proto::BiswapV3, true, false, false),
        // KyberSwap Classic DMM (решение 287): в графе и сигналах (своя формула); исполнение — шаг KYBER_DMM (только v2).
        "kyber_dmm" => (PoolKind::AmpV2, Proto::KyberDmm, true, false, false),
        // SmarDex (решение 288): в графе и сигналах (своя формула по времени блока); исполнение — шаг SMARDEX (только v2).
        "smardex" | "smardex_exact_equality" => {
            (PoolKind::Fictive, Proto::Smardex, true, false, false)
        }
        // Balancer V2 (решение 289): в графе и сигналах (формулы пулов); исполнение — шаг BALANCER (только v2).
        "balancer_v2" | "balancer_v2_wallet" => (PoolKind::VaultPool, Proto::BalancerV2, true, false, false),
        // Balancer V3 (решение 441): в графе и сигналах (расчёт хранилища V3); исполнение — шаг BALANCER_V3 (с v4).
        "balancer_v3" => (PoolKind::VaultPool, Proto::BalancerV3, true, false, false),
        // Liquidity Book V2.1 (решение 290): в графе и сигналах (порт getSwapOut); исполнение — шаг LB (только v2).
        "lb_v21" | "lb_v21_clone" => (PoolKind::LbBin, Proto::LbV21, true, false, false),
        // Curve Twocrypto-NG / Tricrypto-NG (решение 442): в графе и сигналах (порт get_y и комиссии пула); исполнение —
        // шаг STABLE_U256 контракта-исполнителя (exchange с индексами uint256).
        "curve_twocrypto_ng" => (PoolKind::Crypto, Proto::CurveTwocrypto, true, false, false),
        "curve_tricrypto_ng" => (PoolKind::Crypto, Proto::CurveTricrypto, true, false, false),
        // Curve crypto v1 (решение 453): фабрика crypto v1 и tricrypto 2021 — в графе и сигналах; исполнение — шаг
        // STABLE_U256 исполнителя (exchange(uint256, uint256, dx, 0) у обоих есть).
        "curve_crypto_v1" => (PoolKind::Crypto, Proto::CurveCryptoV1, true, false, false),
        "curve_tricrypto_v1" => (
            PoolKind::Crypto,
            Proto::CurveTricryptoV1,
            true,
            false,
            false,
        ),
        // Решение 472 (T472-10): двухмонетный crypto-пул без фабрики (EURS/2CRV) — протокол tricrypto 2021 из списка,
        // вариант CryptoPair (две монеты, math TwoV1).
        "curve_crypto2_v1" => (
            PoolKind::Crypto,
            Proto::CurveTricryptoV1,
            true,
            false,
            false,
        ),
        // Camelot V2 (решение 443): в графе и сигналах (своя математика пары); исполнение — шаг SOLIDLY исполнителя.
        "camelot_v2" => (PoolKind::Solidly, Proto::CamelotV2, true, false, false),
        // Решение 470: Aero Basic — в графе и сигналах (математика Solidly); исполнение — шаг SOLIDLY исполнителя.
        "aero_basic" => (PoolKind::Solidly, Proto::AeroBasic, true, false, false),
        // Liquidity Book v2.0 (решение 445): в графе и сигналах (порт swap v2.0); исполнение — шаг LB20 исполнителя.
        "lb_v20" | "sectorone_lb_v20" => (PoolKind::LbBin, Proto::LbV20, true, false, false),
        // Liquidity Book V2.2 (решение 446): тот же путь, что у lb_v21 (порт getSwapOut + откат MAX_LIQUIDITY_PER_BIN), шаг LB.
        "lb_v22" | "sectorone_lb_v22" => (PoolKind::LbBin, Proto::LbV22, true, false, false),
        // Ekubo (решение 447): в графе и сигналах (порт swap ядра); исполнение — шаг EKUBO исполнителя.
        "ekubo_v2_router" => (PoolKind::Ekubo, Proto::Ekubo, true, true, true),
        "ekubo" => (PoolKind::Ekubo, Proto::Ekubo, true, false, false),
        // PancakeSwap StableSwap, Lista StableSwap, Saddle (Nerve) — решение 122 F, 3: в графе и сигналах; Universal
        // Router их не исполняет (tradable = false); исполнение — шагом контракта-исполнителя: STABLE_U256 у pancake / lista
        // (exchange с индексами uint256), SADDLE у saddle (swap uint8 + deadline) — exec_kind.rs (решение 122 X, стейблсвапы).
        "pancake_stableswap" => (
            PoolKind::StableSwap,
            Proto::PancakeStable,
            true,
            false,
            false,
        ),
        "lista_stableswap" | "lista_stableswap_proxy" => {
            (PoolKind::StableSwap, Proto::ListaStable, true, false, false)
        }
        "saddle_stableswap" => (
            PoolKind::StableSwap,
            Proto::SaddleStable,
            true,
            false,
            false,
        ),
        // WooFi WooPPV2 (решение 122 F, 3): в графе и сигналах; своей математики нет (развёрнутый код закрыт) — котировка
        // по лесенке ответов квотера контракта (dex.pmm_ladder_usd, quote/pmm_ladder.rs); шага WOO нет до сухой сделки.
        "baibai_module" => (PoolKind::OraclePmm, Proto::BaiBai, true, true, true),
        "baibai" => (PoolKind::OraclePmm, Proto::BaiBai, true, false, false),
        "lunarbase" => (PoolKind::OraclePmm, Proto::Lunar, true, false, false),
        "lunarbase_module" => (PoolKind::OraclePmm, Proto::Lunar, true, true, true),
        "elfomofi_module" => (PoolKind::OraclePmm, Proto::Elfomo, true, true, true),
        "elfomofi" => (PoolKind::OraclePmm, Proto::Elfomo, true, false, false),
        "woofi_v2" => (PoolKind::OraclePmm, Proto::WooPP, true, false, false),
        // Решение 484: Wombat Exchange — в графе и сигналах (порт CoreV2 + haircut + высокое покрытие); исполнение — шаг
        // WOMBAT исполнителя v5 (exec_kind.rs).
        "wombat" => (PoolKind::StableSwap, Proto::Wombat, true, false, false),
        // Fluid DEX T1 (решение 440): в графе и сигналах (порт _swapIn); исполнение — шаг FLUID исполнителя.
        "fluid_t1" => (PoolKind::SmartColDebt, Proto::FluidT1, true, false, false),
        // Скважина Basin (решение 485): в графе и сигналах (порт функций скважины); исполнение — шаг BASIN исполнителя v5.
        "basin_well" => (PoolKind::BasinWell, Proto::BasinWell, true, false, false),
        // Shell Protocol Proteus (решение 486): в графе и сигналах (порт Proteus.sol); исполнение — шаг SHELL исполнителя.
        "shell_proteus" => (PoolKind::Proteus, Proto::ShellProteus, true, false, false),
        // Slipstream (Topaz v3 на BSC; решения 80, 85): форк Uniswap V3 core — в графе, котируется общей математикой V3
        // (math/swap_cl), НО tradable=false — исполнения пока нет (роутер Slipstream — следующая фаза). graphable=true,
        // tradable=false, fork=false (ни UR своей фабрики, ни свой роутер-форк пока не подключены): как THENA Algebra /
        // Solidly — в графе и для сигналов, но guard executable (quote/graph_route.rs, cycle.rs) по family.tradable в
        // авто-сделку не пускает. Вид ядра — V3 (PoolState::V3), чтение/дискавери/квотер — свои (два отличия от Uni V3).
        // Решение 470: Aero Slipstream V3 — тот же CLPool (вариант SlipstreamAero: комиссия крючка, pools/aero_cl.rs).
        // Решение 483: Ramses CL V2 — тот же протокол (вариант RamsesClV2: свои slot0 / ticks / Mint / PoolCreated и
        // FeeAdjustment; pools/ramses.rs, load_ramses.rs).
        "slipstream"
        | "slipstream_aero"
        | "slipstream_aero_metaquoter"
        | "slipstream_dynamic_fee"
        | "solidly_v3"
        | "slipstream_ramses" => (PoolKind::V3, Proto::Slipstream, true, false, false),
        "solidly_v3_module" => (PoolKind::V3, Proto::Slipstream, true, true, true),
        // Maverick V2 (решение 122 F): в графе и сигналах (порт расчёта команды Maverick); исполнение — шаг MAVERICK
        // контракта-исполнителя (exec_kind::via_executor), своего пути через Universal Router нет.
        "maverick_v2" => (PoolKind::TickBin, Proto::Maverick, true, false, false),
        // PancakeSwap Infinity Stable (решение «122 (Infinity Stable)»): в графе и исполним Universal Router своей DEX
        // (tradable, не форк) — пул живёт в синглтоне CLPoolManager; решение 395 — ещё шаг INF_CL исполнителя v3.
        "pancake_infinity_stable" => (
            PoolKind::StableSwap,
            Proto::InfinityStable,
            true,
            true,
            false,
        ),
        _ => return None,
    })
}

/// Вид пула DEX по id и варианту dex.kind. None — вариант неизвестен или «не поддерживается». Одна запись на
/// сочетание (id, протокол, graphable, tradable, fork, вид шага исполнителя); решение 409 — таблица без замка на чтение
/// (pools/family_intern.rs). Ошибок нет.
pub fn make(id: &str, kind: &str) -> Option<&'static Family> {
    with_variant(id, kind, super::family_variant::of(kind))
}

fn configured(ctx: &Ctx, id: &str, kind: &str) -> Option<&'static Family> {
    if ctx.chain_id() == Some(8453) && kind == "solidly_v1" {
        return with_variant(id, kind, Variant::SolidlyClassic);
    }
    make(id,kind)
}

fn with_variant(id: &str, kind: &str, variant: Variant) -> Option<&'static Family> {
    let (pk, proto, graphable, tradable, fork) = spec(kind)?;
    let executor_kind = exec_kind::of(kind);
    Some(super::family_intern::intern(
        id,
        |f| {
            f.proto == proto
                && f.graphable == graphable
                && f.tradable == tradable
                && f.fork == fork
                && f.executor_kind == executor_kind
                && f.variant == variant
        },
        |id| Family {
            id,
            kind: pk,
            proto,
            title: id,
            graphable,
            tradable,
            fork,
            executor_kind,
            variant,
            event: super::event_flags::EventFlags::new(),
        },
    ))
}

/// Все DEX с поддержанным видом в настройках (включённые и нет), по id. Решение 435 (433-з): только ключи области
/// «DEX» этой площадки (ctx_place.rs own_dex: сеть по умолчанию — без «@», другая — «@сеть»); сеть площадки не
/// определить (несколько площадок DEX без dex.chain / dex.default_chain) — пусто. Ошибок нет.
pub fn all(ctx: &Ctx) -> Vec<&'static Family> {
    let Ok(suffix) = ctx.dex_suffix() else {
        return Vec::new();
    };
    let mut out: Vec<&'static Family> = ctx
        .settings
        .all()
        .into_iter()
        .filter(|(name, _, _)| name == "dex.kind")
        .filter_map(|(_, scope, v)| match scope {
            Scope::Dex(id) if own_dex(&id, suffix.as_deref()) => configured(ctx,&id, v.as_str()?),
            _ => None,
        })
        .collect();
    out.sort_by_key(|f| f.id);
    out
}

/// Подключена ли DEX индексатора. Принимает: id DEX. Возвращает: её вид или причину «DEX X не подключена: …».
/// Ошибок нет (причина — текст для интерфейса).
pub fn classify(ctx: &Ctx, id: &str) -> Result<&'static Family, String> {
    let no = |why: String| format!("DEX {id} не подключена: {why}");
    let kind = ctx
        .dex_text("dex.kind", id)
        .map_err(|_| no("нет в таблице сопоставления (dex.kind)".into()))?;
    let f = configured(ctx,id, &kind).ok_or_else(|| no(format!("вид «{kind}» — не поддерживается")))?;
    // graphable=false — вид только в справочнике (например V3-форк): не отслеживается и не в графе. tradable отдельно
    // (решение 82): V2-форк graphable, но не исполняется — это не повод не подключать его к графу/сигналам; допуск к
    // сделке проверяет quote/graph_route::executable (маршрут через форк → сигнал, авто-сделкой не исполняется).
    if !f.graphable {
        return Err(format!(
            "DEX {id} не подключена: вид «{kind}» — только справочник, в граф маршрутов пока не берётся (решение 59)"
        ));
    }
    if f.variant == Variant::RingFew {
        super::ring::active(ctx, f.id).map_err(|e| no(e.message))?;
    }
    if f.variant.registered_v2_router() && !f.variant.dual_v2_execution() {
        ctx.addr("dex.multicall3").map_err(|e| no(e.message))?;
    }
    if f.variant.factory_quoter() && !super::base_native_v3::valid(ctx, f) {
        return Err(no(
            "сеть, фабрика или квотер расходятся с профилем V3 Base".into()
        ));
    }
    if (f.direct_module() && f.fork)
        || (f.variant == Variant::FraxTwamm && f.executor_kind.is_some())
    {
        super::direct_module::configured(ctx, id).map_err(|e| no(e.message))?;
    }
    match ctx.dex_flag("dex.pool_enabled", id) {
        Ok(true) => {}
        Ok(false) => return Err(no("выключена (dex.pool_enabled)".into())),
        Err(_) => return Err(no("не задана dex.pool_enabled".into())),
    }
    for name in required(ctx, f) {
        if ctx
            .settings
            .get_exact(name, &Scope::Dex(id.into()))
            .is_none()
        {
            return Err(no(format!("не задана {name}")));
        }
    }
    Ok(f)
}

/// Настройки DEX, без которых её пул нельзя проверить, слушать или исполнить (по виду). Universal Router и Permit2
/// нужны только для исполнения (tradable): у форка (tradable=false) их нет — ему достаточно фабрики, роутера и
/// источника комиссии, чтобы проверить пул, слушать и сверить с его собственным роутером (getAmountsOut). V2-форку без
/// роутера (v2_fork_executor, решение 280) роутер не нужен: сверка — обмен через исполнитель в симуляции. Ошибок нет.
pub fn required(ctx: &Ctx, f: &Family) -> Vec<&'static str> {
    let mut list = Vec::new();
    if f.variant == super::family_variant::Variant::AlgebraSingleV19 {
        list.extend(["dex.factory_runtime_hash","dex.pool_code_hashes","dex.plugin_runtime_hashes","dex.fusion_next_block_s"]);
    }
    if f.variant == super::family_variant::Variant::ElasticV5 {
        if f.tradable {list.extend(["dex.elastic_native_router","dex.router","dex.module_runtime_hashes","dex.pmm_module_bindings"]);}
        list.extend(["dex.factory_runtime_hash", "dex.router_runtime_hash", "dex.quoter_runtime_hash", "dex.tick_reader_runtime_hash"]);
    }
    // Universal Router и Permit2 — только у НЕ-форка (поддержанного DEX): форк исполняется своим роутером (dex.router)
    // с прямым ERC-20 approve, UR/Permit2 ему не нужны (решения 59, 82).
    if f.tradable && !f.fork {
        list.extend([
            "dex.universal_router",
            "dex.universal_router_version",
            "dex.permit2",
        ]);
    }
    match f.proto {
        Proto::V2 if f.variant == Variant::OmniV2 => {
            list.extend(["dex.factory","dex.pool_code_hashes","dex.v2_fee_source","dex.v2_fee_pct",
                         "dex.router_runtime_hash","dex.permit2_runtime_hash"]);
        }
        Proto::V2 if f.variant == Variant::EtherVista => {
            list.extend(["dex.factory","dex.factory_runtime_hash","dex.quoter","dex.router_runtime_hash",super::verify_algebra_dir::SETTING]);
        }
        Proto::V2 if f.variant == Variant::Oasis => {
            list.extend(["dex.factory","dex.factory_runtime_hash","dex.oasis_mev_slot",super::verify_algebra_dir::SETTING]);
            if f.executor_kind.is_none(){ list.extend(["dex.quoter","dex.router_runtime_hash"]); }
            // У dual-профиля два независимо проверяемых способа. load принимает хотя бы один.

        }
        Proto::V2 if f.variant == Variant::DeltaSwap => {
            list.extend([
                "dex.factory",
                "dex.router",
                "dex.router_runtime_hash",
                super::verify_algebra_dir::SETTING,
            ]);
        }
        Proto::V2 if f.variant.frax_twamm() => {
            list.extend(["dex.factory", "dex.quoter"]);
            if f.variant == Variant::FraxTwammRouter {
                list.push("dex.fraxswap_router_hash");
            }
            if f.executor_kind.is_some() {
                list.extend(["dex.router", "dex.module_runtime_hashes"]);
            }
        }
        Proto::V2 if f.variant.umbrae() => {
            list.extend(["dex.factory","dex.factory_runtime_hash","dex.pair_runtime_hash","dex.umbrae_roles"]);
            if f.variant==Variant::UmbraeDlmm {list.extend(["dex.quoter","dex.pmm_ladder_usd"]);}
            else {list.extend(["dex.proxy_implementations","dex.proxy_runtime_hashes"]);}
            if f.tradable {list.extend(["dex.router","dex.module_runtime_hashes"]);}
        }
        Proto::V2 if f.variant == Variant::Antfarm => list.extend(["dex.factory","dex.quoter"]),
        Proto::V2 if f.variant == Variant::Temple => {
            list.extend(["dex.quoter","dex.router_runtime_hash","dex.pair_runtime_hash"]);
            if f.executor_kind.is_some(){list.extend(["dex.router","dex.module_runtime_hashes"]);}
        }
        Proto::V2 if matches!(f.variant,Variant::MintClub | Variant::Carbon) => {
            list.extend(["dex.factory","dex.factory_runtime_hash","dex.proxy_implementations","dex.proxy_runtime_hashes"]);
        }
        Proto::V2 if f.variant == Variant::Virtuals => {
            list.extend(["dex.factory","dex.factory_runtime_hash","dex.proxy_implementations","dex.proxy_runtime_hashes","dex.quoter","dex.bonding_roles"]);
            if f.tradable { list.extend(["dex.router","dex.module_runtime_hashes","dex.pmm_module_bindings"]); }
        }
        Proto::V2 if f.variant == Variant::Gemswap => {
            list.extend(["dex.factory","dex.factory_runtime_hash","dex.proxy_implementations","dex.proxy_runtime_hashes"]);
        }
        Proto::V2 if f.variant == Variant::BunnySwap => {
            list.extend(["dex.bunny_pair","dex.bunny_token","dex.quoter","dex.quoter_runtime_hash","dex.pair_runtime_hash"]);
            if f.tradable { list.extend(["dex.router","dex.module_runtime_hashes"]); }
        }
        Proto::V2 if f.variant == Variant::Memebox => {
            list.extend(["dex.factory", "dex.factory_runtime_hash", "dex.pool_code_hashes",
                "dex.v2_fee_source", "dex.v2_fee_method", "dex.v2_fee_denominator"]);
            if f.tradable { list.extend(["dex.router","dex.module_runtime_hashes"]); }
        }
        Proto::V2 => {
            list.push("dex.factory");
            if matches!(
                f.variant,
                Variant::V2Create2
                    | Variant::CandySwap
            | Variant::V2Create2LiveFee
                    | Variant::DiamondV2
                    | Variant::V2RouterCreate2
                    | Variant::V2DualCreate2
                    | Variant::V2DualCreate2LiveFee
                    | Variant::WhiteSwap
            ) {
                list.push(super::verify_algebra_dir::SETTING);
            }
            if f.variant == Variant::WhiteSwap {
                list.extend(["dex.proxy_implementations", "dex.proxy_runtime_hashes"]);
            }
            if f.variant == Variant::V2DualCreate2LiveFee { list.extend(["dex.pair_runtime_hash","dex.factory_runtime_hash"]); }
            if f.variant == Variant::CandySwap && f.tradable {list.push("dex.module_runtime_hashes");}
            if f.variant == Variant::KewlV2 { list.push("dex.pair_runtime_hash"); }
            if f.variant.diamond_router() {
                list.extend(["dex.factory_runtime_hash","dex.proxy_implementations","dex.proxy_runtime_hashes"]);
            }
            if f.variant.live_v2_fee() {
                list.push("dex.v2_fee_rule_hash");
            }
            // Роутер — только у вида со своим путём исполнения (PancakeSwap/Uniswap V2, v2_fork: сверка getAmountsOut и
            // обмен роутером форка). v2_fork_executor (решение 280, tradable=false) роутера не имеет: сверка и обмен —
            // через контракт-исполнитель (quote/v2_pair_quote.rs).
            if f.tradable && !matches!(f.variant, Variant::WhiteSwap | Variant::DiamondV2 | Variant::KewlV2 | Variant::V2DualCreate2 | Variant::V2DualCreate2LiveFee) {
                list.push("dex.router");
            }
            match ctx.dex_text("dex.v2_fee_source", f.id).as_deref() {
                Ok("pool" | "factory") => {
                    list.extend(["dex.v2_fee_method", "dex.v2_fee_denominator"])
                }
                Ok(_) => list.push("dex.v2_fee_pct"),
                Err(_) => list.push("dex.v2_fee_source"),
            }
        }
        // Решение 483: Ramses CL — тики прямыми геттерами пула (load_ramses.rs, TickLens не нужен); пул зовёт
        // ramsesV2SwapCallback (RamsesV2Pool.sol L1238) — без dex.v3_swap_callback исполнитель не проверить.
        Proto::UniswapV3 if f.variant == Variant::RamsesCl => {
            list.extend(["dex.factory", "dex.quoter", "dex.v3_swap_callback"])
        }
        Proto::UniswapV3 | Proto::PancakeV3 if f.variant.registered_v3() => {
            list.extend(["dex.factory","dex.factory_runtime_hash","dex.pool_code_hashes","dex.tick_lens","dex.quoter","dex.quoter_runtime_hash","dex.v3_quoter_method"]);
            if matches!(f.variant, Variant::HelloV3 | Variant::RegisteredUniV3) { list.push("dex.v3_swap_callback"); }
            if f.tradable {list.extend(["dex.router","dex.module_runtime_hashes"]);}
        }
        Proto::PancakeV3 if f.variant == Variant::BaseNativeV3 => {
            list.extend([
                "dex.factory",
                "dex.tick_lens",
                "dex.quoter",
                "dex.v3_quoter_method",
            ]);
            if f.tradable {
                list.push("dex.router");
            }
        }
        Proto::PancakeV3 | Proto::UniswapV3 => {
            list.extend(["dex.factory", "dex.tick_lens", "dex.quoter"])
        }
        Proto::InfinityCl | Proto::InfinityBin => {
            list.extend(["dex.pool_manager", "dex.vault", "dex.quoter"])
        }
        Proto::UniswapV4 if f.variant.launch() => list.extend(["dex.pool_manager","dex.quoter","dex.state_view","dex.launch_roles","dex.launch_pool_keys"]),
        Proto::UniswapV4 if f.variant == Variant::DeliCl => list.extend([
            "dex.pool_manager","dex.state_view","dex.position_manager","dex.quoter","dex.deli_role_bindings","dex.deli_fee_currency",
        ]),
        Proto::UniswapV4 if f.variant == Variant::DeliConstant => list.extend([
            "dex.pool_manager","dex.state_view","dex.quoter","dex.deli_role_bindings",
        ]),
        Proto::UniswapV4 => list.extend([
            "dex.pool_manager",
            "dex.position_manager",
            "dex.state_view",
            "dex.quoter",
        ]),
        // KyberSwap Elastic (решение 81): фабрика (проверка пула getPool(t0,t1,swapFeeUnits)), свой роутер (форк
        // исполняется им, UR/Permit2 не нужны — отсечены выше), квотер (сверка QuoterV2.quoteExactInputSingle),
        // читатель тиков (TicksFeesReader.getTicksInRange — связный список тиков). Решение 475: длина части чтения тиков
        // dex.elastic_ticks_chunk (pools/elastic_ticks.rs) — без неё тиков не прочесть, DEX не подключается.
        Proto::KyberElastic => list.extend([
            "dex.factory",
            "dex.router",
            "dex.quoter",
            "dex.tick_reader",
            "dex.elastic_ticks_chunk",
        ]),
        // DODO PMM (решение 86b): только фабрика DVM/DPP/DSP — ею проверяется пул (getDODOPoolBidirection,
        // решение 59) и слушаются события создания (NewDVM/NewDPP/NewDSP). Квотер — сам пул (адрес из справочника),
        // отдельной настройки dex.quoter не нужно. Роутера нет (tradable=false, исполнение — 86c).
        Proto::DodoDvm | Proto::DodoDpp | Proto::DodoDsp => list.push("dex.factory"),
        // Algebra (THENA Fusion, решения 80, 81, фаза B): фабрика (проверка пула poolByPair, решение 59; события
        // создания Pool) и квотер (сверка QuoterV2.quoteExactInputSingle без параметра fee, решение 73). Своего
        // читателя тиков нет — сжатый битмап tickTable (решение 463) читается прямо у пула (load_algebra.rs). Роутера нет
        // (tradable=false — исполнение отсечено выше, UR/Permit2 не добавлены).
        Proto::Algebra => list.extend(["dex.factory", "dex.quoter"]),
        // Algebra Integral (решение 188): фабрика (poolByPair, событие Pool — как V1.9); квотер — виртуальный
        // (dex.quoter_virtual = да убирает dex.quoter из списка, virtual_quoter.rs); без него пул не подключается.
        Proto::AlgebraIntegral => {
            list.extend(["dex.factory", "dex.quoter"]);
            if matches!(f.variant,Variant::BasePluginV1 | Variant::TrebleIntegral | Variant::SupernovaIntegral) {
                list.extend(["dex.factory_runtime_hash","dex.plugin_factory","dex.plugin_runtime_hashes",super::verify_algebra_dir::SETTING]);
            }
            if matches!(
                f.variant,
                Variant::QuickswapIntegral | Variant::NamiIntegral
            ) {
                list.extend([
                    "dex.plugin_factory",
                    "dex.plugin_runtime_hashes",
                    "dex.plugin_pool_offset",
                    "dex.plugin_beacon",
                    "dex.plugin_implementation",
                ]);
                if f.variant == Variant::NamiIntegral {
                    list.push("dex.plugin_extension");
                }
            }
        }
        // Решение 444: фабрика (poolByPair, событие Pool — как V1.9); квотер — виртуальный (dex.quoter_virtual = да).
        // Ревью 444: допустимые хеши кода пула (сверка версии по CREATE2 — verify_algebra_dir.rs).
        Proto::AlgebraDirFee => list.extend(["dex.factory", "dex.pool_code_hashes", "dex.quoter"]),
        // Solidly (Topaz v2, THENA V1; решения 80, 85): только фабрика — ею проверяется пул (getPool(coin, quote,
        // stable), решение 59), читается комиссия (getFee(pool, stable), load_solidly.rs) и слушается событие создания
        // (PoolCreated). Квотер — сам пул (getAmountOut; адрес из справочника), отдельной настройки dex.quoter нет.
        // Роутера нет (tradable=false — исполнение отсечено выше, UR/Permit2 не добавлены).
        // Solidly V1 (THENA V1, решение 122 F) — то же: фабрика getPair(a, b, stable) / getFee(stable) / PairCreated.
        // Решение 273: задан источник комиссии dex.solidly_fee_source — ещё правило и то, что нужно источнику (метод
        // пула / методы фабрики по типу; делитель — при правиле numerator). Не задан — только фабрика, как раньше.
        Proto::SolidlyV1 if f.variant == Variant::LeetSwap => list.push("dex.factory"),
        Proto::Solidly | Proto::SolidlyV1 => {
            list.push("dex.factory");
            if f.variant == Variant::SolidlyProxy {list.extend(["dex.pair_runtime_hash","dex.proxy_implementations","dex.proxy_runtime_hashes"]);}
            if f.variant == Variant::SolidlyGenerator {list.extend(["dex.solidly_generator_runtime_hash","dex.solidly_pause_method"]);}
            if f.variant == Variant::SolidlyProxy {list.extend(["dex.factory_runtime_hash", "dex.pool_code_hashes"]);}
            else if f.variant.solidly_classic() {
                list.extend(["dex.factory_runtime_hash","dex.pool_code_hashes","dex.solidly_fee_source"]);
            }
            if let Ok(src) = ctx.dex_text("dex.solidly_fee_source", f.id) {
                list.push("dex.solidly_fee_rule");
                match src.as_str() {
                    "pool" | "factory_address" => list.push("dex.solidly_fee_method"),
                    "factory_by_type" => list.extend([
                        "dex.solidly_fee_method_stable",
                        "dex.solidly_fee_method_volatile",
                    ]),
                    _ => {}
                }
                if matches!(
                    ctx.dex_text("dex.solidly_fee_rule", f.id).as_deref(),
                    Ok("numerator")
                ) {
                    list.push("dex.solidly_fee_denominator");
                }
            }
            // Решение 483: у пары Ramses Legacy нет getFee(bool) фабрики THENA — комиссия только fee() пары
            // (Pair.sol L56), источник обязателен.
            let no_source = ctx.dex_text("dex.solidly_fee_source", f.id).is_err();
            if f.variant == Variant::RamsesLegacy && no_source {
                list.push("dex.solidly_fee_source");
            }
        }
        // StableSwap (решение 122 F): фабрики нет — пул подтверждает список адресов из официальной документации DEX.
        Proto::CurveStable => list.push("dex.stable_pools"),
        // Curve StableSwap-NG (решение 275): реестр фабрики NG (get_coins / is_meta) подтверждает пул; версии реализации
        // пула, с которыми сверена математика (version() пула — verify_curve_ng.rs).
        Proto::CurveNg => list.extend(["dex.factory", "dex.curve_ng_versions"]),
        // Nomiswap Stable (решение 284): фабрика — реестр getPair (проверка пары), поиск по getPair и событие PairCreated.
        Proto::NomiStable => list.push("dex.factory"),
        // Metric OMM (решение 285): фабрика (isPool, getImmutables().factory); квотер — только виртуальный
        // (dex.quoter_virtual = да убирает dex.quoter, как у Algebra Integral): своего квотера в официальном SDK для BSC нет.
        Proto::MetricOmm => list.extend(["dex.factory", "dex.quoter"]),
        // iZiSwap / Biswap V3 (решение 286): фабрика (factory() пула и реестр pool(x, y, fee)); квотер — только
        // виртуальный (dex.quoter_virtual = да убирает dex.quoter): официальный квотер в код не подключён.
        Proto::IziSwap if f.variant==Variant::IziModules => list.extend(["dex.factory","dex.quoter","dex.factory_runtime_hash","dex.pair_runtime_hash","dex.pool_code_hashes","dex.izi_module_bindings"]),
        Proto::IziSwap | Proto::BiswapV3 => list.extend(["dex.factory", "dex.quoter"]),
        // KyberSwap Classic DMM (решение 287): фабрика (factory() пула, реестр getPools, PoolCreated) и квотер сверки —
        // DMMRouter02 (getAmountsOut по пути из одного пула; адрес — официальная документация KyberSwap).
        Proto::KyberDmm => list.extend(["dex.factory", "dex.quoter"]),
        // SmarDex (решение 288): фабрика (getPair / PairCreated — как V2); квотер — только виртуальный (quoteSmardex,
        // dex.quoter_virtual = да убирает dex.quoter): официального квотера пары у SmarDex нет.
        Proto::Smardex => list.extend(["dex.factory", "dex.quoter"]),
        // Balancer V2 (решение 289): хранилище (getPool / getPoolTokens, обмен и квотер queryBatchSwap) и фабрика весовых
        // пулов (isPoolFromFactory). Фабрика стейблсвапов dex.balancer_stable_factory — по желанию (не задана — только
        // весовые пулы).
        Proto::BalancerV2 => {
            list.extend(["dex.vault", "dex.factory"]);
            if f.variant == Variant::BalancerVaultWallet { list.push("dex.router_runtime_hash"); }
        }
        // Balancer V3 (решение 441): хранилище (состояние, обмен), роутер V3 — квотер сверки querySwapSingleTokenExactIn
        // (dex.quoter), списки фабрик по выпускам (пустой список — у DEX нет фабрик этого выпуска).
        Proto::BalancerV3 => {
            list.extend(["dex.vault", "dex.quoter"]);
            // Решение 452: все списки фабрик по выпускам (balancer_v3_factories::LISTS) и списки крючков комиссии.
            list.extend(super::balancer_v3_factories::LISTS.iter().map(|(n, _)| *n));
            list.extend(["dex.bv3_stable_surge_hooks", "dex.bv3_mev_capture_hooks"]);
        }
        // Liquidity Book V2.1 (решение 290): фабрика (getFactory пула и реестр getLBPairInformation). Квотер сверки — сам
        // пул getSwapOut (адрес не нужен).
        Proto::LbV21 => {
            list.push("dex.factory");
            if f.variant == Variant::LbV21Clone {
                list.push("dex.lb_implementations");
            }
        }
        // Fluid DEX T1 (решение 440): фабрика (isDex) и резолвер FluidDexReservesResolver в dex.quoter — им читается
        // состояние (getPoolReservesAdjusted) и сверяется выход (estimateSwapIn).
        Proto::FluidT1 => list.extend(["dex.factory", "dex.quoter"]),
        // Shell Protocol Proteus (решение 486): учётный контракт Ocean (dex.vault) и список пулов из документации
        // (dex.stable_pools); списки реализаций (dex.proteus_utility_v1 / _v2) проверяются у каждого пула.
        Proto::ShellProteus => list.extend(["dex.vault", "dex.stable_pools"]),
        // Curve Twocrypto-NG / Tricrypto-NG (решение 442): реестр фабрики (get_coins, find_pool_for_coins), версии
        // реализации пула и адреса математики, с исходником которых сверены формулы (verify_crypto.rs).
        Proto::CurveTwocrypto | Proto::CurveTricrypto => list.extend([
            "dex.factory",
            "dex.curve_crypto_versions",
            "dex.curve_crypto_math",
        ]),
        // Curve crypto v1 фабрики (решение 453): реестр фабрики (get_coins, find_pool_for_coins) и адреса шаблонов
        // пула, с исходником которых сверена формула (код прокси EIP-1167 пула).
        Proto::CurveCryptoV1 => list.extend(["dex.factory", "dex.curve_crypto_v1_templates"]),
        // Tricrypto 2021 (решение 453): фабрики нет — список адресов пулов из журнала развёртывания curvefi.
        Proto::CurveTricryptoV1 => list.push("dex.stable_pools"),
        // Camelot V2 (решение 443): фабрика (factory() пары, реестр getPair, событие PairCreated — как V2). Квотер сверки —
        // сама пара getAmountOut (адрес не нужен).
        Proto::CamelotV2 => list.push("dex.factory"),
        // Решение 470: Aero Basic — фабрика своей кривой (реестр getPool, PoolCreated, getBaseFee, isPaused,
        // mevTaxModule). Квотер сверки — сам пул getAmountOut.
        // Ревью t240: и закреплённые модули комиссии и MEV-налога фабрики (load_aero_basic.rs, aero_mev.rs pinned).
        Proto::AeroBasic => {
            list.extend(["dex.factory", "dex.aero_fee_module", "dex.aero_mev_module"])
        }
        // Liquidity Book v2.0 (решение 445): фабрика (factory() пула и реестр getLBPairInformation) и квотер сверки —
        // LBRouter v2.0 getSwapOut(пара, вход, swapForY): у пары v2.0 своего getSwapOut нет.
        Proto::LbV20 if f.variant == Variant::SectorLb => list.extend(["dex.factory","dex.quoter","dex.router","dex.router_runtime_hash","dex.lb_implementations"]),
        Proto::LbV22 if f.variant == Variant::SectorLb => list.extend(["dex.factory","dex.router","dex.router_runtime_hash","dex.lb_implementations"]),
        Proto::LbV20 => list.extend(["dex.factory", "dex.quoter"]),
        // Liquidity Book V2.2 (решение 446): ещё реализации пула, сверенные с исходником (implementation() пула).
        Proto::LbV22 => list.extend(["dex.factory", "dex.lb_implementations"]),
        // Ekubo (решение 447): ядро (состояние, события, обмен), PoolKeyIndex (ключ пула по PoolId), слова битмапа и
        // квотер сверки (Router.quote — MEVCaptureRouter). Расширение MEVCapture — по желанию (не задано — пулы с ним
        // не берутся).
        Proto::Ekubo if f.variant == Variant::EkuboV2 => list.extend(["dex.ekubo_core","dex.ekubo_bitmap_words","dex.quoter","dex.factory_runtime_hash","dex.router_runtime_hash"]),
        Proto::Ekubo => list.extend([
            "dex.ekubo_core",
            "dex.ekubo_key_index",
            "dex.ekubo_bitmap_words",
            "dex.quoter",
        ]),
        // Скважина Basin (решение 485): Aquifer (реестр wellImplementation, BoreWell) и допущенные реализации скважин;
        // функции (dex.basin_cp2 / dex.basin_stable2 — хотя бы одна) проверяет pools/basin_conf.rs.
        Proto::BasinWell => list.extend(["dex.factory", "dex.basin_well_impls"]),
        // PancakeSwap / Lista StableSwap: реестр фабрики (getPairInfo / getPairInfos) подтверждает пул.
        Proto::ListaStable if f.variant == Variant::ListaStableProxy => {
            list.extend(["dex.factory", "dex.pool_implementation"])
        }
        Proto::PancakeStable | Proto::ListaStable => list.push("dex.factory"),
        // Saddle (Nerve): фабрики нет — список адресов пулов из официальной документации.
        Proto::SaddleStable => list.push("dex.stable_pools"),
        // Решение 484: Wombat — фабрики нет: пулы — список dex.stable_pools (docs.wombat.exchange), реализации пула —
        // сверенные с исходником (dex.wombat_dynamic_implementations — необязательная: без неё динамические пулы не
        // читаются).
        Proto::Wombat => list.extend(["dex.stable_pools", "dex.wombat_implementations"]),
        // WooFi: общий контракт, оракул и закреплённая реализация прокси (сменилась — семейство стоит до сверки).
        Proto::Lunar => {
            list.extend([
                "dex.pmm_pool",
                "dex.pool_implementation",
                "dex.pmm_counter",
                "dex.pmm_base_tokens",
                "dex.pmm_runtime_hashes",
            ]);
            if f.fork {
                list.extend(["dex.router", "dex.pmm_module_bindings"]);
            }
        }
        Proto::Elfomo | Proto::BaiBai => {
            list.extend([
                "dex.pmm_pool",
                "dex.oracle",
                "dex.pool_implementation",
                "dex.vault",
                "dex.pmm_counter",
                "dex.pmm_base_tokens",
                "dex.pmm_runtime_hashes",
            ]);
            if f.proto == Proto::BaiBai {
                list.push("dex.pmm_aux_implementations");
            } else {
                list.extend(["dex.pmm_pricing_offsets", "dex.pmm_vault_offsets"]);
            }
            if f.fork {
                list.extend(["dex.router", "dex.pmm_module_bindings"]);
            }
        }
        Proto::WooPP => list.extend(["dex.pmm_pool", "dex.oracle", "dex.pool_implementation"]),
        // Slipstream (Topaz v3; решения 80, 85): фабрика (проверка пула getPool(coin, quote, int24 tickSpacing),
        // решение 59; события создания PoolCreated) и квотер (сверка ISlipstreamQuoterV2.quoteExactInputSingle по
        // tickSpacing, решение 73). Своего читателя тиков нет — битмап читается прямо у пула (load_slipstream.rs),
        // dex.tick_lens не нужен. Роутера нет (tradable=false — исполнение отсечено выше, UR/Permit2 не добавлены).
        Proto::Slipstream if f.variant == Variant::SolidlyCl => {
            list.push("dex.factory");
            if f.fork {
                list.push("dex.router");
            }
        }
        Proto::Slipstream => {
            list.extend(["dex.factory", "dex.quoter"]);
            // Решение 470: Aero Slipstream V3 — ещё официальный крючок обмена (pools/aero_cl.rs) и окно первой
            // комиссии нового блока (T470-01, pools/aero_cl_head.rs).
            if matches!(
                f.variant,
                Variant::SlipstreamAero | Variant::SlipstreamAeroMetaquoter
            ) {
                list.extend([
                    "dex.cl_swap_hook",
                    "dex.cl_hook_ahead_s",
                    "dex.aero_mev_module",
                ]);
            }
        }
        // Maverick V2 (решение 122 F): фабрика (isFactoryPool, PoolCreated), квотер (сверка перед сделкой, решение 73),
        // PoolLens и радиус тиков — без них состояние пула не прочитать.
        Proto::Maverick => list.extend([
            "dex.factory",
            "dex.quoter",
            "dex.pool_lens",
            "dex.tick_radius",
        ]),
        // Infinity Stable (решение «122 (Infinity Stable)»): менеджер CL (ключ пула poolIdToPoolKey, события Initialize),
        // Vault (там лежат токены пула — держатель для проб налога), список крючков пулов.
        // Квотер сверки — CLQuoter Infinity (dex.quoter, основной) и get_dy крючка (дополнительный) — решение
        // «122 (Infinity Stable, ответы)».
        Proto::InfinityStable => list.extend([
            "dex.pool_manager",
            "dex.vault",
            "dex.stable_hooks",
            "dex.quoter",
        ]),
    }
    // Решение 186: виртуальный квотер — квотер DEX не нужен (quote/virtual_quoter.rs).
    if !f.variant.external_algebra_swap()
        && super::super::quote::virtual_quoter::on(ctx, f.id, f.proto)
    {
        list.retain(|n| *n != "dex.quoter");
    }
    list
}

pub use super::dex_rules::{for_ui, smart_ok, still};

#[cfg(test)]
mod tests {
    use super::spec;
    use crate::settings::catalog::dex::DEX_KINDS;

    /// Один список вариантов: каждый вариант каталога понятен адаптеру (кроме «не поддерживается»).
    #[test]
    fn kinds_one_source() {
        for k in DEX_KINDS {
            assert_eq!(spec(k).is_some(), *k != "unsupported", "{k}");
        }
        assert!(spec("нет такого").is_none());
    }
}
