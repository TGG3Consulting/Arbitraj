//! Файл: venues/bsc_dex/pools/family_variant.rs
//! Что делает: решение 451 — вариант исходника внутри одного протокола вида пула (Proto): тот же ABI чтения, событий и
//! проверки, но другое правило математики у развёрнутого кода. Вариант задаётся значением dex.kind (отдельное значение
//! на вариант — Тигран выбирает его в интерфейсе, как вид), хранится в виде DEX (Family::variant) и читается
//! математикой на горячем пути без обращения к хранилищу настроек.
//! Кто вызывает: pools/families.rs (make — при создании вида), quote/pool_quote.rs, quote/depth_band.rs, pools/ramses.rs
//! (решение 483).
//! От чего зависит: нет.
//! Настройки: dex.kind [DEX] (значение выбирает вариант).
//!
//! Варианты (официальные исходники — в комментариях вариантов).

/// Вариант исходника вида пула. Base — исходник, по которому вид сделан (у всех прежних dex.kind).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variant {
    /// Исходник, по которому сделан вид (поведение без изменений).
    Base,
    /// Ethereum V2 singleton and its official full-input router.
    EkuboV2,
    /// Hydrex Integral Base: после Swap плагин может обменять собственные комиссии в том же пуле по отдельному тарифу.
    HydrexIntegral,
    /// Nami v4: обновляемый plugin может исполнить дополнительный обмен в стороннем пуле.
    NamiIntegral,
    /// Supernova AlgebraBasePluginV3: скидка tx.origin и внешнее исполнение Reflex.
    SupernovaIntegral,
    CypherIntegral,
    /// Ring Few V2 на Ethereum, отдельный путь underlying через проверенные fw wrappers.
    RingFew,
    /// 9mm, Throne и Omni на Base проверяют свои callback через роутеры фабрик.
    BaseNativeV3,
    /// Кривая Curve торгует IBT и зарегистрированным PT Spectra.
    Spectra,
    IziModules,
    UmbraeDamm,
    UmbraeDlmm,
    HelloV3,
    VvsV3,
    RegisteredV3,
    RegisteredUniV3,
    /// Algebra Integral с проверяемым beacon-прокси плагина QuickSwap V4.
    QuickswapIntegral,
    /// Lista StableSwap с проверенной реализацией EIP-1967 на каждом чтении состояния.
    ListaStableProxy,
    /// Unchain X (UNX-V3-Contracts f6185ae6): slot0, Swap и callback от Uniswap V3, плюс геттер lmPool().
    UniswapV3Lm,
    /// KyberSwap Elastic v1 (dex.kind = kyber_elastic_v1): код фабрики 0x5F1dddbf348aC2fbe22a163e30F99F9ECE3DD50a в любой
    /// сети (KyberNetwork/ks-elastic-sc 4ab08c0a README.md L6-L17 «v1» — в т.ч. Arbitrum и BSC; Sourcify 42161 full
    /// match, solc 0.8.9). Elastic v2 (вид kyber_elastic) — например фабрика 0xC7a590291e07B9fe9E64b86c58fD8fC764308C4A на
    /// BSC (Sourcify 56: SwapMath.sol `usedAmount > specifiedAmount`). Вид выбирается по коду фабрики, а не по сети.
    /// Отличие от вида (HEAD 4ab08c0, Elastic v2) в расчёте сумм обмена одно: SwapMath.computeSwapStep v1 —
    /// `(isExactInput && usedAmount >= specifiedAmount)`, v2 — `>` (ks-elastic-sc 4ab08c0 contracts/libraries/
    /// SwapMath.sol L60; первый коммит v2 0e7713a — ещё `>=`). Остальное (calcReachAmount, estimate/
    /// calcIncrementalLiquidity, calcFinalPrice, calcReturnedAmount, QuadMath, FullMath, MAX_TICK_DISTANCE = 480,
    /// цикл Pool.swap, _getInitialSwapData, _updateLiquidityAndCrossTick, раскладка PoolStorage) — то же. Отличия вне
    /// расчёта сумм: тик после неполного шага (у v1 нет startSqrtP), MIN_LIQUIDITY, длина массива TicksFeesReader.
    ElasticV1,
    ElasticV5,
    /// Решение 470 (T451-01): Aero «Slipstream V3» (dex.kind = slipstream_aero; Dromos-Labs/metadex-slipstream-public
    /// 1.1.0, развёртывание Arbitrum cl-arbitrum.json): тот же CLPool, что у вида slipstream (slot0, ticks, getPool(int24),
    /// PoolCreated, Swap, цикл обмена), но комиссия обмена — от крючка (DynamicSwapFeeHook): меняется по блокам без
    /// событий пула — комиссия читается каждым блоком, крючок пула и правило MEV-налога проверяются (pools/aero_cl.rs).
    SlipstreamAero,
    /// Тот же CLPool и DynamicSwapFeeHook. Официальный Metaquoter выбирает пул по адресу.
    SlipstreamAeroMetaquoter,
    /// Aerodrome Base CLFactory.getSwapFee(pool), читается через CLPool.fee() на блоке.
    SlipstreamDynamicFee,
    SlipstreamUniversal,
    TopazCl,
    /// Solidly V3 Ethereum/Base, slot0 с комиссией, встроенный quoteSwap и свой ABI оплаты.
    SolidlyCl,
    /// V2 с фиксированной реализацией пары: адрес сверяется по хешу байткода создания CREATE2.
    V2Create2,
    /// V2 с проверенным Universal Router формата Omni и его собственным Permit2.
    OmniV2,
    /// Резервы constant-product хука Deli у Uniswap V4.
    DeliConstant,
    LaunchUmia,
    LaunchBaseline,
    Angstrom,
    LaunchDoppler,
    LaunchIndex,
    LaunchO1,
    LaunchO1Wrapped,
    FluxNativeCl,
    FluxNativeBin,
    /// Размещение LB с обычным роутером и зарегистрированным клоном пула.
    SectorLb,
    /// Концентрированная ликвидность Deli с комиссией в wBLT.
    DeliCl,
    /// Пара исполняется родным роутером, обычный 4-аргументный swap V5 недоступен.
    V2RouterCreate2,
    /// Каноническая пара допускает 4-аргументный swap V5 и собственный Router02.
    V2DualCreate2,
    SafeSwapV2,
    SafeSwapProxy,
    SafeSwapTrade,
    DegenSwap,
    ToonSwap,
    V2ExactRouter,
    /// Зарегистрированные Router02 и V5 с изменяемой комиссией пары и проверкой обоих runtime.
    V2DualCreate2LiveFee,
    DeltaSwap,
    Oasis,
    EtherVista,
    /// Solidly V1 с комиссией по получателю и внешним источником фабрики.
    LeetSwap,
    /// Solidly V1 Base: привязка CREATE2 и наблюдаемое обеспечение на каноническом блоке.
    SolidlyClassic,
    /// Создание пар Solidly V1 вынесено в штатный PairGenerator.
    SolidlyGenerator,
    /// Solidly Ethereum V2 delegates factory and pair calls to independently upgradable logic.
    SolidlyProxy,
    SolidlyPairOverride,
    BasePluginV1,
    AlgebraSingleV19,
    TrebleIntegral,
    /// Три обновляемые прокси WhiteSwap с комиссией прямого обмена и скидкой роутера.
    WhiteSwap,
    /// Пара CREATE2 с изменяемой ставкой. Резервы и комиссия сверяются вместе.
    V2Create2LiveFee,
    CandySwap,
    /// Direct Vault.swap with wallet debit and payout checked on the approval and signing path.
    BalancerVaultWallet,
    PuzzleRanges,
    Memebox,
    BunnySwap,
    /// Launch Bonding with virtual reserves and tax on the VIRTUAL side.
    Virtuals,
    MintClub,
    Carbon,
    Temple,
    Dagrow,
    IntegralSize,
    PropVenue,
    RubiconClassic,
    YieldBasis,
    BalancerV1,
    BancorLegacy,
    BancorV3,
    UniswapV1,
    Pendle,
    Stabull,
    MaverickOne,
    MaverickTwoRouter,
    Surge,
    Antfarm,
    Bundles,
    /// EIP-2535 с отдельной привязкой исполняемого селектора фабрики и Router02.
    DiamondV2,
    /// Пара проверяется по фабрике CREATE и коду исполнения; общий прокси с Router02.
    KewlV2,
    Gemswap,
    /// Fraxswap V2 исполняет долгосрочные ордера к timestamp блока перед обменом.
    FraxTwamm,
    /// Одиночное исполнение официальным роутером с проверкой выхода получателя.
    FraxTwammRouter,
    /// Curve StableSwap 2021 (dex.kind = curve_stableswap_plain, решение 472): протокол Proto::CurveStable (геттеры
    /// coins/balances/fee, get_dy и exchange с int128 — те же), но A — `A_precise()` (A·A_PRECISION, A_PRECISION = 100),
    /// несходимость D/y — откат (`raise`), у пула из двух монет D_P — одной строкой `D·D/x0·D/x1/4`. Исходники: 2pool
    /// Arbitrum 0x7f90122BF0700F9E7e1F688fe926940E8839F353 (Vyper 0.2.12, Sourcify 42161 match) и Plain2Basic /
    /// Plain3Basic / Plain4Basic фабрики Curve Sidechain/L2 (curvefi/curve-factory f389123, Vyper 0.2.15; Sourcify
    /// 42161 match реализаций 0x54e8…7A62, 0xe381…7A62, 0x8d53…fa86). Чтение — load_curve_plain.rs, проверка —
    /// verify_curve_plain.rs, события — events_curve.rs (topics_of), состояние — каждым блоком (per_block.rs).
    CurvePlain,
    /// Метапул фабрики Curve Sidechain/L2 (dex.kind = curve_stableswap_meta, решение 472, доделка): тот же протокол и
    /// шаблон, что CurvePlain, но курс монеты 1 (LP-токен базового пула) — get_virtual_price() базового пула на блоке,
    /// D_P — циклом. Исходник — MetaUSD.vy / MetaUSDBalances.vy (curvefi/curve-factory f389123); чтение —
    /// load_curve_meta.rs, проверка — verify_curve_plain.rs (реализация из dex.curve_plain_implementations и
    /// get_base_pool фабрики = coins(1)), события — events_curve.rs, состояние — каждым блоком (per_block.rs).
    CurveMeta,
    /// Пул Curve StableSwap с курсом оракула и нативным ETH (dex.kind = curve_stableswap_oracle, решение 481, T472-10):
    /// ETH/wstETH Arbitrum 0x6eB2dc694eB516B16Dc9FBc678C60052BbdD7d80 (Vyper 0.3.1, Sourcify 42161 creationMatch match,
    /// runtimeMatch null; docs/sources/curve_arbitrum/). Протокол Proto::CurveStable (coins/balances/fee, A_precise,
    /// exchange(int128, int128, dx, min_dy) — те же), пул — из списка dex.stable_pools; отличие от CurvePlain: курс
    /// монеты 1 — `Oracle(ORACLE).latestAnswer()` на блоке (`_stored_rates` L193-L199; адрес — `oracle()` пула L851,
    /// подтверждается проверкой), курс монеты 0 (метка ETH 0xEeee…, `__init__` L144) — 10^18, D_P — циклом (get_D
    /// L213-L239). Чтение — load_curve_oracle.rs, проверка — verify_curve_plain.rs, состояние — каждым блоком (курс
    /// оракула меняется без событий пула, per_block.rs), шаг — CURVE_ETH (v5).
    CurveOracle,
    /// Двухмонетный crypto-пул Curve первого поколения без фабрики (dex.kind = curve_crypto2_v1, решение 472, T472-10):
    /// протокол Proto::CurveTricryptoV1 (пул из списка dex.stable_pools, монеты — coins(k), is_killed), но монет две и
    /// математика — CurveCryptoSwap2 (CryptoMath::TwoV1). Исходник — EURS/2CRV Arbitrum
    /// 0xA827a652Ead76c6B0b3D19dba05452E06e25c27e: curvefi/curve-crypto-contract 41e6289 «deployment-logs/2021-10-30.
    /// EURS/CryptoSwap.vy» (Vyper 0.3.0; адрес — deployment.log того же каталога): geometric_mean, newton_D, newton_y,
    /// halfpow, get_xcp, _A_gamma — тот же текст, что CurveCryptoSwap2ETH.vy (curve-factory-crypto 7ed1011; сверка
    /// функций скриптом), exchange L686-L765 — те же действия, что `_exchange` 2ETH (+ assert not is_killed L688).
    /// Чтение — load_crypto_v1.rs (n = 2 и is_killed()), модель tweak_price v1 фабрики к нему не применяется.
    CryptoPair,
    /// Решение 483: Ramses CL — старая CL-линия (dex.kind = v3_fork_ramses; пул RamsesV2Pool за RamsesBeaconProxy,
    /// реализация 0x6553aAEE5a3A482A7d61bb5e093B05140Fe17e21, Sourcify 42161 match, solc 0.7.6). Протокол — Uniswap V3
    /// (slot0 с uint8 feeProtocol L239-L263, Swap / Mint / Burn — IRamsesV2PoolEvents.sol L21-L80, getPool(a, b, fee)
    /// фабрики). Отличия: комиссия обмена — `currentFee()` (L216-L218, states.fee в computeSwapStep L1044-L1056), а
    /// `fee()` — начальная (L199-L201, ключ реестра фабрики); меняется setFee фабрики (ProtocolActions.sol L75-L85,
    /// событие пула FeeAdjustment(uint24, uint24) L26, L84); ticks() — 10 полей (L338-L370, liquidityNet — 2-е);
    /// обратный вызов обмена — `ramsesV2SwapCallback` (L1238, L1256). Чтение — load_ramses.rs, события — ramses.rs.
    RamsesCl,
    /// Решение 483: Ramses CL V2 — линия V3 (dex.kind = slipstream_ramses; RamsesV3Pool — исходник в Sourcify 42161 match
    /// RamsesV3PoolDeployer 0xb722efaAbe807FAeA16068f595EaA9aa1a62CECD, solc 0.8.28). Протокол — Slipstream (getPool(a, b,
    /// int24 tickSpacing), QuoterV2 по tickSpacing, uniswapV3SwapCallback L662, L671). Отличия: slot0 из 7 полей (uint24
    /// feeProtocol, L817-L842), ticks() из 8 полей (L896-L920), PoolCreated фабрики — подпись Uniswap V3
    /// (IRamsesV3Factory.sol L13-L15), Mint с полем `uint256 index` (IRamsesV3PoolEvents.sol L22-L31), комиссия `fee()`
    /// (L798-L800) меняется setFee (ProtocolActions.sol L80-L87: событие пула FeeAdjustment(uint24, uint24)).
    RamsesClV2,
    /// Решение 483: Ramses Legacy V2 — форк Solidly (dex.kind = solidly_v1_ramses; PairFactory
    /// 0xADd32480630A16dfAcEe6eeFcB3ab2181449Dc3B, Sourcify 42161 match, solc 0.8.30, contracts/Pair.sol). Протокол —
    /// Solidly V1 (metadata L479-L493, getPair(a, b, bool) PairFactory.sol L30, _k / _f / _d / _get_y L405-L449 — те же
    /// формулы, что THENA V1, math/solidly_v1_math.rs). Отличия: getAmountOut = `_getAmountOut(...) - 1` (L452-L458),
    /// комиссия — `fee()` пары над FEE_DENOM = 1 000 000 (L56, L63; настройки dex.solidly_fee_*), Sync(uint112, uint112)
    /// (IPair.sol L15), PairCreated(token0, token1, pair, uint256) без stable (IPairFactory.sol L5).
    RamsesLegacy,
    /// Ethereum SmardexFactory 0xB878…214F: approxEq(x,x) возвращает true и при округлении допуска до нуля.
    SmardexExactEquality,
    /// LB V2.1 с проверкой неизменяемого шаблона клона по коду пары.
    LbV21Clone,
}

impl Variant {
    pub(crate) fn protected_router_wallet(self)->bool{matches!(self,Self::SafeSwapTrade|Self::DegenSwap)}
    pub fn launch(self)->bool {matches!(self,Self::Angstrom|Self::LaunchUmia|Self::LaunchBaseline|Self::LaunchDoppler|Self::LaunchIndex|Self::LaunchO1|Self::LaunchO1Wrapped)}
    pub fn flux_native(self) -> bool { matches!(self, Self::FluxNativeCl | Self::FluxNativeBin) }
    pub fn native_market_module(self) -> bool { self == Self::LaunchO1Wrapped || self.flux_native() }
    pub fn deli(self)->bool{matches!(self,Self::DeliConstant|Self::DeliCl)}
    /// У шага 17 адрес отдельного рынка закодирован как poolId, а назначение — модуль фабрики.
    pub fn pool_address_step(self)->bool{self.registered_module() || matches!(self,Self::Stabull | Self::MaverickOne)}
    /// Величина платы меняется по tx.origin и может измениться между двумя шагами в одном пуле.
    pub fn origin_fee(self)->bool{self==Self::TopazCl}
    pub fn registered_module(self) -> bool { matches!(self, Self::RegisteredV3 | Self::VvsV3) }
    pub fn registered_v3(self) -> bool { matches!(self, Self::RegisteredV3 | Self::RegisteredUniV3 | Self::HelloV3 | Self::VvsV3) }
    pub fn factory_quoter(self)->bool { self == Self::BaseNativeV3 || self.registered_v3() }
    pub fn umbrae(self)->bool { matches!(self,Self::UmbraeDamm|Self::UmbraeDlmm) }
    pub fn wallet_module(self)->bool { matches!(self,Self::Stabull|Self::MaverickOne|Self::UmbraeDlmm|Self::UmbraeDamm|Self::Memebox|Self::RegisteredV3|Self::VvsV3|Self::BunnySwap|Self::CandySwap) }
    pub(crate) fn exact_router(self)->bool {matches!(self,Self::MaverickTwoRouter|Self::SlipstreamUniversal|Self::CypherIntegral|Self::RegisteredUniV3|Self::VvsV3|Self::SolidlyProxy|Self::SolidlyPairOverride|Self::V2ExactRouter)}
    pub(crate) fn solidly_classic(self)->bool {matches!(self,Self::SolidlyClassic | Self::SolidlyGenerator | Self::SolidlyProxy | Self::SolidlyPairOverride)}
    pub(crate) fn external_algebra_swap(self)->bool {matches!(self, Self::NamiIntegral | Self::SupernovaIntegral)}

    pub(crate) fn diamond_router(self)->bool {matches!(self,Self::DiamondV2 | Self::KewlV2)}

    pub(crate) fn live_v2_fee(self) -> bool {
        matches!(self, Self::V2Create2LiveFee | Self::V2DualCreate2LiveFee | Self::CandySwap | Self::DiamondV2)
    }

    pub fn frax_twamm(self) -> bool {
        matches!(self, Self::FraxTwamm | Self::FraxTwammRouter)
    }
    pub fn dual_v2_execution(self) -> bool {
        matches!(self,Self::Gemswap | Self::V2ExactRouter | Self::V2DualCreate2 | Self::SafeSwapV2 | Self::ToonSwap | Self::V2DualCreate2LiveFee) || self.diamond_router()
    }
    pub fn registered_v2_router(self) -> bool {
        matches!(self, Self::V2RouterCreate2 | Self::V2ExactRouter | Self::V2DualCreate2 | Self::SafeSwapV2 | Self::ToonSwap | Self::V2DualCreate2LiveFee | Self::DiamondV2 | Self::DeltaSwap)
    }

    /// Резервы и комиссия этих V2-пар должны обновляться одним чтением состояния блока.
    pub fn v2_block_state(self) -> bool {
        matches!(
            self,
            Self::DegenSwap | Self::SafeSwapTrade | Self::Gemswap | Self::ToonSwap | Self::V2Create2LiveFee | Self::V2DualCreate2LiveFee | Self::CandySwap | Self::DiamondV2 | Self::FraxTwamm | Self::FraxTwammRouter | Self::Memebox | Self::DeltaSwap | Self::Oasis | Self::EtherVista
        )
    }
}

/// Вариант по значению dex.kind. Принимает: значение dex.kind. Возвращает: вариант (неизвестное — Base). Ошибок нет.
pub fn of(kind: &str) -> Variant {
    match kind {
        "ekubo_v2_router" => Variant::EkuboV2,
        "solidly_v1_generator" => Variant::SolidlyGenerator,
        "ring_few_v2" => Variant::RingFew,
        "lb_v21_clone" => Variant::LbV21Clone,
        "smardex_exact_equality" => Variant::SmardexExactEquality,
        "kyber_elastic_v1" => Variant::ElasticV1,
        "kyber_elastic_v5" | "kyber_elastic_module" => Variant::ElasticV5,
        "slipstream_aero" => Variant::SlipstreamAero,
        "slipstream_aero_metaquoter" => Variant::SlipstreamAeroMetaquoter,
        "topaz_cl" => Variant::TopazCl,
        "slipstream_dynamic_fee" => Variant::SlipstreamDynamicFee,
        "slipstream_dynamic_ur" => Variant::SlipstreamUniversal,
        "solidly_v3" | "solidly_v3_module" => Variant::SolidlyCl,
        "curve_stableswap_plain" => Variant::CurvePlain,
        "curve_stableswap_meta" => Variant::CurveMeta,
        "curve_stableswap_oracle" => Variant::CurveOracle,
        "curve_crypto2_v1" => Variant::CryptoPair,
        "algebra_integral_treble" => Variant::TrebleIntegral,
        "algebra_v19_single" => Variant::AlgebraSingleV19,
        "algebra_integral_base_plugin_v1" => Variant::BasePluginV1,
        "algebra_integral_hydrex" => Variant::HydrexIntegral,
        "algebra_integral_nami" => Variant::NamiIntegral,
        "algebra_integral_cypher" => Variant::CypherIntegral,
        "algebra_integral_supernova" => Variant::SupernovaIntegral,
        "algebra_integral_quickswap" => Variant::QuickswapIntegral,
        "lista_stableswap_proxy" => Variant::ListaStableProxy,
        "spectra_stableswap_ng" | "spectra_crypto_v1" => Variant::Spectra,
        "izi_static_modules" => Variant::IziModules,
        "umbrae_dlmm" | "umbrae_dlmm_module" => Variant::UmbraeDlmm,
        "umbrae_damm" | "umbrae_damm_module" => Variant::UmbraeDamm,
        "vvs_v3" | "vvs_v3_module" => Variant::VvsV3,
        "hello_v3" => Variant::HelloV3,
        "registered_uni_v3" => Variant::RegisteredUniV3,
        "registered_v3" | "registered_v3_module" => Variant::RegisteredV3,
        "base_native_v3" | "base_native_v3_module" => Variant::BaseNativeV3,
        "v3_fork_ramses" => Variant::RamsesCl,
        "v3_fork_uniswap_lm" => Variant::UniswapV3Lm,
        "fraxswap_twamm_router" => Variant::FraxTwammRouter,
        "fraxswap_twamm" | "fraxswap_twamm_v5" => Variant::FraxTwamm,
        "ethervista_native" | "ethervista_v5" => Variant::EtherVista,
        "oasisswap_v2_router" | "oasisswap_v2_v5" => Variant::Oasis,
        "v2_deltaswap_router" => Variant::DeltaSwap,
        "v2_fork_router_create2" => Variant::V2RouterCreate2,
        "toonswap" => Variant::ToonSwap,
        "safeswap_v2" => Variant::SafeSwapV2,
        "degenswap_module" => Variant::DegenSwap,
        "safeswap_trade_module" => Variant::SafeSwapTrade,
        "safeswap_proxy_v5" => Variant::SafeSwapProxy,
        "v2_fork_dual_create2" => Variant::V2DualCreate2,
        "v2_fork_dual_router01" => Variant::V2ExactRouter,
        "v2_fork_dual_create2_live_fee" => Variant::V2DualCreate2LiveFee,
        "v2_whiteswap" => Variant::WhiteSwap,
        "sectorone_lb_v20" | "sectorone_lb_v22" => Variant::SectorLb,
        "launch_umia" => Variant::LaunchUmia,
        "launch_baseline" => Variant::LaunchBaseline,
        "angstrom_l1" => Variant::Angstrom,
        "launch_doppler" => Variant::LaunchDoppler,
        "launch_index" => Variant::LaunchIndex,
        "launch_o1" => Variant::LaunchO1,
        "omni_flux_native_cl" => Variant::FluxNativeCl,
        "omni_flux_native_bin" => Variant::FluxNativeBin,
        "launch_o1_wrapped" => Variant::LaunchO1Wrapped,
        "deli_v4_cl" => Variant::DeliCl,
        "deli_v4_constant" => Variant::DeliConstant,
        "v2_omni_universal" => Variant::OmniV2,
        "v2_fork_executor_create2" => Variant::V2Create2,
        "v2_diamond_router" => Variant::DiamondV2,
        "v2_diamond_create" => Variant::KewlV2,
        "gemswap_v2" => Variant::Gemswap,
        "antfarm" => Variant::Antfarm,
        "bundles_swap" | "bundles_swap_v5" => Variant::Bundles,
        "stabull" | "stabull_module" => Variant::Stabull,
        "maverick_v2_router" => Variant::MaverickTwoRouter,
        "maverick_v1" | "maverick_v1_module" => Variant::MaverickOne,
        "surge_native" | "surge_v5" => Variant::Surge,
        "rubicon_classic" => Variant::RubiconClassic,
        "yield_basis" => Variant::YieldBasis,
        "balancer_v1" => Variant::BalancerV1,
        "bancor_legacy" => Variant::BancorLegacy,
        "bancor_v3" => Variant::BancorV3,
        "uniswap_v1" => Variant::UniswapV1,
        "lambda_propamm" => Variant::PropVenue,
        "integral_size" | "integral_size_v5" => Variant::IntegralSize,
        "pendle_market" | "pendle_market_v5" => Variant::Pendle,
        "dagrow" | "dagrow_v5" => Variant::Dagrow,
        "temple" | "temple_v5" => Variant::Temple,
        "carbon_v1" => Variant::Carbon,
        "mint_club_v2" => Variant::MintClub,
        "virtuals_bonding" | "virtuals_bonding_module" | "virtuals_unicorn" | "virtuals_unicorn_module" => Variant::Virtuals,
        "bunnyswap" | "bunnyswap_module" => Variant::BunnySwap,
        "memebox" | "memebox_v5" => Variant::Memebox,
        "candyswap_v2" | "candyswap_module" => Variant::CandySwap,
        "puzzle_ranges" => Variant::PuzzleRanges,
        "balancer_v2_wallet" => Variant::BalancerVaultWallet,
        "v2_fork_executor_create2_live_fee" => Variant::V2Create2LiveFee,
        "slipstream_ramses" => Variant::RamsesClV2,
        "solidly_v2_proxy" => Variant::SolidlyProxy,
        "solidly_v1_pair_override" => Variant::SolidlyPairOverride,
        "leetswap_v2" => Variant::LeetSwap,
        "solidly_v1_ramses" => Variant::RamsesLegacy,
        _ => Variant::Base,
    }
}

#[cfg(test)]
mod tests {
    use super::{Variant, of};

    /// Прежние значения dex.kind остаются Base (поведение BSC не меняется), v1 — свой вариант.
    #[test]
    fn variant_by_kind() {
        assert_eq!(of("kyber_elastic"), Variant::Base);
        assert_eq!(of("saddle_stableswap"), Variant::Base);
        assert_eq!(of("kyber_elastic_v1"), Variant::ElasticV1);
        assert_eq!(of("slipstream"), Variant::Base);
        assert_eq!(of("slipstream_aero"), Variant::SlipstreamAero);
        assert_eq!(of("curve_stableswap"), Variant::Base);
        assert_eq!(of("curve_stableswap_plain"), Variant::CurvePlain);
        assert_eq!(of("curve_stableswap_meta"), Variant::CurveMeta);
        assert_eq!(of("curve_stableswap_oracle"), Variant::CurveOracle);
        assert_eq!(of("curve_crypto2_v1"), Variant::CryptoPair);
        assert_eq!(of("curve_tricrypto_v1"), Variant::Base);
        assert_eq!(of("v3_fork"), Variant::Base);
        assert_eq!(of("v3_fork_ramses"), Variant::RamsesCl);
        assert_eq!(of("slipstream_ramses"), Variant::RamsesClV2);
        assert_eq!(of("solidly_v1"), Variant::Base);
        assert_eq!(of("solidly_v1_ramses"), Variant::RamsesLegacy);
    }
}
