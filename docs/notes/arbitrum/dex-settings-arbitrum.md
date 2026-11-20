# Настройки области «DEX» площадки arb_dex — DEX Arbitrum One с видом пула в коде (решение 448, 20.11.2026)

**Решение 450 (20.11.2026, ветка t226/arb-dex-code-a):** код T448-01…05 сделан — статусы ниже обновлены (пометка
«решение 450»; ревью 450: везде, где код есть, — «код есть, сверка — T450-01 (запись)», не «готово»: записи узла ещё нет): PancakeSwap V2 / V3 —
код есть, сверка — T450-01 (запись) (`pancakeswap_v3`), iZiSwap — код есть, сверка — T450-01 (запись) (поиск по фабрике), KyberSwap Classic Static Fee и
Legacy — код есть, сверка — T450-01 (запись) (дочерние ключи dex.sub_dex), Balancer V2 — + 2Tokens, weighted v2/v3/v4, stable v2
(dex.balancer_math), Curve — механизм есть, 2pool / старые plain — T450-02, crypto — T433-F3.

Задача: для каждого DEX Arbitrum One со статусом «вид есть в коде» (docs/notes/arbitrum/dex-coverage-arbitrum.md,
задачи T433-*) — полный набор настроек области «DEX», как у DEX BSC: вид, адреса, правило комиссии, включение. Код не
менялся. Задачи кода и проверки — docs/TASKS.md, блок «Решение 448».

## 0. Правила и источники

**Ключ области «DEX».** `src/venues/bsc_dex/ctx_place.rs:65-73, 100-106` (433-з): у площадки не сети по умолчанию ключ
= «id индексатора» + «@» + dex.chain площадки. У arb_dex dex.chain по плану — `arbitrum` (catalog/dex.rs:94), ключ —
`<id GeckoTerminal>@arbitrum`, например `uniswap_v3_arbitrum@arbitrum` (тест ctx_place.rs:124-130). Свои ключи
площадка берёт по суффиксу (own_dex, ctx_place.rs:110-115). У DEX, которых нет в GeckoTerminal (DODO, Saddle, WOOFi,
iZiSwap), id вводит Тигран («Добавить DEX», решение 128, pools/dex_ids.rs) — тоже с `@arbitrum`; ниже id — предложение,
имя выбирает Тигран.

**Какие настройки нужны виду** — `pools/families.rs:490-634` (required) и classify (`families.rs:455-484`): dex.kind,
dex.pool_enabled = да и все адреса вида; у видов tradable и не форк (pancakeswap_v2/v3, uniswap_v2/v3/v4) — ещё
dex.universal_router, dex.universal_router_version, dex.permit2 (`families.rs:494-500`). dex.quoter_virtual = да убирает
dex.quoter (`families.rs:630-632`). dex.v3_fee_tiers / dex.cl_tick_spacings / dex.pair_lookup_max — только для поиска
по фабрике (index.factory_scan, catalog/dex.rs:171-172, 184). Поиск по фабрике умеет только виды из
`pools/factory_calls.rs:72-103` (V2, SmarDex, Nomi, DMM, Solidly, Algebra/Integral, DODO, Pancake/Lista Stable, WooFi,
V3 по уровням, Slipstream, Maverick); Curve 2020 / Saddle — списком dex.stable_pools (решение 292).

**dex.pool_enabled** — флаг включения (catalog/dex.rs:100). Ниже «да» — значение для включения; включает Тигран.

**Настройки площадки (не область «DEX»)** — dex.integral_tick_steps, dex.bin_depth_usd, dex.quoter_max_diff_pct,
index.factory_scan и т.п. (область V) — у arb_dex задаются отдельно, в этот документ не входят. Попутно: обёртка
нативной монеты Arbitrum — WETH `0x82aF49447D8a07e3bd95BD0d56f35241523fBab1` (Uniswap docs, v3-arbitrum-deployments,
таблица «Wrapped Native Token»; тот же weth9 в параметрах UR 2.0.0, см. ниже) — это dex.native_wrapped площадки.

**Источники (только официальные, прочитаны 20.11.2026).** arbiscan.io и публичные узлы Arbitrum из песочницы закрыты
(прокси: 403) — верифицированный исходник взят с **Sourcify** (sourcify.dev/server/v2/contract/42161/<адрес>,
через WebFetch), развёртывания и адреса — из официальных репозиториев GitHub (клон, коммит указан) и страниц
документации. Значения on-chain (eth_call) в этой проверке не читались — где нужны, это отдельная проверка своим узлом
(TASKS 448).

Репозитории (коммит):
- Uniswap/docs `d49ac3efc2ed8c8291aaf0e0194fd9c5b3c5d55a` — content/protocols/v2/deployments.mdx,
  content/protocols/v3/deployments/v3-arbitrum-deployments.mdx, content/protocols/v4/deployments.mdx;
- Uniswap/universal-router `3be05ae0` (deploy-addresses/arbitrum.json), тег `2.0.0` = `3663f6db`
  (script/deployParameters/DeployArbitrum.s.sol L10-L15);
- Uniswap/v3-core тег v1.0.0 (contracts/UniswapV3Factory.sol L26-L30), Uniswap/v2-core master
  (contracts/UniswapV2Pair.sol L180-L182);
- pancakeswap/pancake-developer `d620e1d0` (docs/pages/contracts/v2|v3|universal-router|permit2|stableswap/*.md),
  pancakeswap/pancake-v3-contracts `98684794` (projects/v3-core/contracts/PancakeV3Factory.sol L41-L53);
- sushi-labs/sushi `76d9cd5c` (src/evm/config/features/sushiswap-v2.ts, sushiswap-v3.ts, pancakeswap-v2.ts,
  uniswap-v2.ts), sushiswap/v2-core `a349882c` (contracts/UniswapV2Pair.sol L193-L194);
- KyberNetwork/ks-elastic-sc `4ab08c0a` (README.md L6-L17);
- balancer/balancer-deployments `b55dc2b8` (v2/tasks|deprecated/*/output/arbitrum.json);
- lfj-gg/joe-sdk-v2 `a6f5a8c3` (src/constants/v2Addrs.ts);
- SmarDex-Dev/smart-contracts `7ab2beb2` (deployments/arbitrum, deployments/bsc);
- curvefi/stableswap-ng `3332cd44` (scripts/deployments.py L24-L32, scripts/set_up_base_pools.py L74-L79,
  contracts/main/CurveStableSwapNG.vy L216);
- saddle-finance/saddle-contract `2cfdd055` (deployments/arbitrum_mainnet);
- woonetwork/docs `9bcc162d` (references/readme/arbitrum-one.md L5, bnb-smart-chain.md), woonetwork/WooPoolV2 `f73a0f64`;
- izumiFinance/izumi-docs `652b2692` (docs/source/iZiSwap/deployed_contracts/mainnet.rst L53-L110).

Страницы документации (WebFetch 20.11.2026): docs.kyberswap.com/liquidity-solutions/kyberswap-classic/contracts/classic-contract-addresses;
docs.mav.xyz/technical-reference/contract-addresses/v2-contract-addresses; docs.camelot.exchange/contracts/arbitrum/one-mainnet;
docs.dodoex.io/en/developer/contracts/dodo-v1-v2/contracts-address/arbitrum.

Статусы: **готово** — все обязательные настройки вида имеют значение из официального источника и исходник развёртывания
сверен с видом (в «оговорке» — что ещё проверить своим узлом при включении); **нужен код** — без изменения кода DEX не
подключить; **не подтверждено** — нет официального значения или исходник не сверен (почему — в строке).

## 1. Сводка

| № задачи | Ключ области «DEX» | dex.kind | Статус |
| --- | --- | --- | --- |
| T433-061 | `uniswap-v2-arbitrum@arbitrum` | uniswap_v2 | **готово** |
| T433-002 | `uniswap_v3_arbitrum@arbitrum` | uniswap_v3 | **готово** |
| T433-073 | `uniswap-v4-arbitrum@arbitrum` | uniswap_v4 | **готово** |
| T433-001 | `sushiswap_arbitrum@arbitrum` | v2_fork | **готово** |
| T433-044 (новое) | `sushiswap-v3-arbitrum@arbitrum` | v3_fork | **готово** (семейство установлено этой проверкой) |
| T433-007 | `kyberswap_classic_arbitrum@arbitrum` | kyber_dmm | **готово** (DMM; решение 450: Static Fee и Legacy — код есть, сверка — T450-01 (запись), дочерние ключи, §2.6) |
| T433-051 | `smardex-arbitrum@arbitrum` | smardex | **готово** |
| T433-074 | `maverick-v2-arbitrum@arbitrum` | maverick_v2 | **готово** |
| T433-084 | `camelot-v4@arbitrum` | algebra_integral | **готово** |
| T433-034 | `traderjoe-v2-1-arbitrum@arbitrum` | lb_v21 | **готово** (оговорка — реализация пары, T448-11) |
| T433-014 | `balancer_arbitrum@arbitrum` | balancer_v2 | **готово частично** (решение 450: 20210418 / 20210624 — готово; 2Tokens / weighted v2–v4 / stable v2 — код есть, сверка — T450-01 (запись), дочерние ключи, §2.11; composable stable, meta-stable, LBP, linear, managed — нужен код, T450-03…07) |
| T433-005 | `curve_arbitrum@arbitrum` | curve_stableswap_ng | **готово частично** (пулы фабрики NG; решение 450: механизм дочерних ключей есть; 2pool / старые plain — нужен код, T450-02; crypto — T433-F3) |
| T433-086 | `dodo-amm-dvm@arbitrum`, `dodo-amm-dpp@arbitrum`, `dodo-amm-dsp@arbitrum` (id — предложение) | dodo_dvm / dodo_dpp / dodo_dsp | **готово** (пулы — только поиском по фабрике) |
| T433-098 | `iziswap@arbitrum` (id — предложение) | iziswap | **код есть, сверка — T450-01 (запись)** (решение 450: поиск по фабрике pool(x, y, fee), §3.1) |
| T433-049 | `pancakeswap-v2-arbitrum@arbitrum` | pancakeswap_v2 | **код есть, сверка — T450-01 (запись)** (решение 450: dex.universal_router_version = `pancakeswap_v3`, §4.1) |
| T433-050 | `pancakeswap-v3-arbitrum@arbitrum` | pancakeswap_v3 | **код есть, сверка — T450-01 (запись)** (решение 450, §4.1) |
| T433-064 | `pancakeswap-stableswap-arbitrum@arbitrum` | pancake_stableswap | **не подтверждено** — нет официального адреса фабрики Arbitrum; у SmartRouter и v3 UR Arbitrum stableFactory = 0 (T448-10, решение 451, §4.2) |
| T433-006 | `kyberswap_elastic_arbitrum@arbitrum` | kyber_elastic_v1 | **готово** (решение 451: вариант вида v1; оговорка — запись узла tools/arbitrum_record/kyber_elastic_v1.py; включать ли — Тигран, §4.3) |
| T433-083 | `aero-slipstream-arbitrum@arbitrum` | — (новое семейство) | **нужен код** — развёртывание Arbitrum найдено (Dromos-Labs/metadex-slipstream-public, «Slipstream V3»), комиссия — от крючка обмена, вид slipstream не подходит (T448-09, решение 451, §4.4) |
| T433-087 | `saddle-finance@arbitrum` (id — предложение) | saddle_stableswap | **готово** (решение 451: математика = Nerve; swapStorage 7 полей и FlashLoan — исправлено в виде; оговорка — запись tools/arbitrum_record/saddle.py, §4.5) |
| T433-095 | `woofi-swap@arbitrum` (id — предложение) | woofi_v2 | **готово** (решение 451: оракул = WooracleV2_2 Sourcify, state() как в виде; реализация 0x7b5f…4281 по Sourcify proxyResolution — оговорка: слот и селекторы своим узлом, tools/arbitrum_record/woofi.py, §4.6) |

Решение 451 (20.11.2026): Kyber Elastic, Saddle, WOOFi → **готово** (с оговорками записи узла), Aero Slipstream →
**нужен код** (новое семейство), PancakeSwap StableSwap — **не подтверждено**. Итог после 451: **готово 15**, **нужен код
2**, **не подтверждено 4** (PancakeSwap V2, V3, StableSwap; плюс Sushi V3 — готово, 16-й).

Итог решения 448: из 20 DEX с видом в коде (15 GeckoTerminal + Curve частично + 4 DefiLlama) — **готово 12**, **нужен код 1**,
**не подтверждено 7**; плюс Sushi V3 (T433-044) — семейство установлено, **готово** (13-й). Готовы частично (пулы
других семейств под тем же id) — Curve, Balancer V2, KyberSwap Classic: задачи кода T448-02…04.
Решение 450: PancakeSwap V2, PancakeSwap V3, iZiSwap, KyberSwap Static Fee / Legacy — код есть, сверка — T450-01 (запись);
Balancer V2 и Curve — частично (нужен код: T450-02…07).

## 2. Готово к включению настройками

### 2.1 Uniswap V2 — `uniswap-v2-arbitrum@arbitrum` (T433-061)

| Настройка | Значение | Источник |
| --- | --- | --- |
| dex.kind | `uniswap_v2` | вид пула = UniswapV2Pair: pairInitCodeHash Arbitrum `0x96e8ac4277198ff8b6f785478aa9a39f403cb768dd02cbee326c3e7da348845f` (universal-router 2.0.0, DeployArbitrum.s.sol L14) = UNISWAP_V2_INIT_CODE_HASH (sushi uniswap-v2.ts L23-L24, один на все сети) — тот же код пары v2-core |
| dex.factory | `0xf1D7CC64Fb4452F05c498126312eBE29f30Fbcf9` | Uniswap/docs v2/deployments.mdx L18; тот же v2Factory в UR 2.0.0 L12 |
| dex.router | `0x4752ba5dbc23f44d87826276bf6fd6b1c372ad24` | Uniswap/docs v2/deployments.mdx L18 |
| dex.v2_fee_source | `fixed` | комиссия пары — константа кода |
| dex.v2_fee_pct | `0.3` | v2-core UniswapV2Pair.sol L180-L182: `balance0.mul(1000).sub(amount0In.mul(3))` |
| dex.universal_router | `0xa51afafe0263b40edaef0df8781ea9aa03e381a3` | Uniswap/docs v3-arbitrum-deployments.mdx L28, v4/deployments.mdx L82; universal-router deploy-addresses/arbitrum.json: «UniversalRouterV2» |
| dex.universal_router_version | `2.0` | deploy-addresses/arbitrum.json: этот адрес = UniversalRouterV2 (2.0); 2.1.1 — `0x8B844f88…`, 2.1.2 — `0x2d014117…` (там же). У BSC uniswap — тоже UR V2 0x1906c1d6 (docs/notes/reflection-facts-2026-11-10.md:14). UR 2.0.0 знает v2Factory и v3Factory этой DEX (DeployArbitrum.s.sol L12-L15) |
| dex.permit2 | `0x000000000022D473030F116dDEE9F6B43aC78BA3` | v3-arbitrum-deployments.mdx L27; UR 2.0.0 L10 |
| dex.pool_enabled | да | включение — Тигран |

### 2.2 Uniswap V3 — `uniswap_v3_arbitrum@arbitrum` (T433-002)

| Настройка | Значение | Источник |
| --- | --- | --- |
| dex.kind | `uniswap_v3` | docs ссылаются на uniswap-v3-core v1.0.0 (v3-arbitrum-deployments.mdx L14); poolInitCodeHash Arbitrum `0xe34f199b…8b54` (UR 2.0.0 L15) — тот же, что у Ethereum: один код пула |
| dex.factory | `0x1F98431c8aD98523631AE4a59f267346ea31F984` | v3-arbitrum-deployments.mdx L14 |
| dex.tick_lens | `0xbfd8137f7d1516D3ea5cA83523914859ec47F573` | там же L17 |
| dex.quoter | `0x61fFE014bA17989E743c5F6cB21bF9697530B21e` (QuoterV2) | там же L25 |
| dex.universal_router / _version / dex.permit2 | как у 2.1 (`0xa51afafe…`, `2.0`, `0x0000…22D473`) | там же L27-L28 |
| dex.v3_fee_tiers (необяз., только поиск по фабрике) | `500, 3000, 10000` | v3-core v1.0.0 UniswapV3Factory.sol L26-L30 (конструктор). Уровень 100 включается отдельно (enableFeeAmount) — на Arbitrum не проверен (T448-12) |
| dex.pool_enabled | да | — |

### 2.3 Uniswap V4 — `uniswap-v4-arbitrum@arbitrum` (T433-073)

| Настройка | Значение | Источник |
| --- | --- | --- |
| dex.kind | `uniswap_v4` | Uniswap/docs v4/deployments.mdx L73-L85 (ссылки на v4-core / v4-periphery) |
| dex.pool_manager | `0x360e68faccca8ca495c1b759fd9eee466db9fb32` | v4/deployments.mdx L76 |
| dex.position_manager | `0xd88f38f930b7952f2db2432cb002e7abbf3dd869` | там же L78 |
| dex.state_view | `0x76fd297e2d437cd7f76d50f01afe6160f86e9990` | там же L80 |
| dex.quoter | `0x8928074CA1b241D8Ec02815881c1Af11E8bC5219` (V4Quoter) | там же L79 |
| dex.universal_router / _version / dex.permit2 | `0xa51afafe…`, `2.0`, `0x0000…22D473` | там же L82, L85; UR 2.0.0 знает v4PoolManager `0x360E68fa…` (DeployArbitrum.s.sol тега 2.0.0, L16) |
| dex.pool_enabled | да | — |

### 2.4 SushiSwap V2 — `sushiswap_arbitrum@arbitrum` (T433-001)

| Настройка | Значение | Источник |
| --- | --- | --- |
| dex.kind | `v2_fork` | init code hash пары Arbitrum `0xe18a34eb0e04b04f7a0ac29a6e80748dca96319b42c54d679cb821dca90c6303` — тот же, что у всех сетей Sushi V2 (sushi sushiswap-v2.ts L67-L82); пара — UniswapV2Pair sushiswap/v2-core |
| dex.factory | `0xc35dadb65012ec5796536bd9864ed8773abc74c4` | sushi sushiswap-v2.ts L149 |
| dex.router | `0x1b02da8cb0d097eb8d57a175b88c7d8b47997506` | sushi sushiswap-v2.ts L189 |
| dex.v2_fee_source | `fixed` | — |
| dex.v2_fee_pct | `0.3` | sushiswap/v2-core UniswapV2Pair.sol L193-L194 (`mul(1000).sub(amount0In.mul(3))`); SDK sushiswap-v2-pool.ts L154 (`_997`) |
| dex.pool_enabled | да | — |

### 2.5 SushiSwap V3 — `sushiswap-v3-arbitrum@arbitrum` (T433-044, семейство установлено)

| Настройка | Значение | Источник |
| --- | --- | --- |
| dex.kind | `v3_fork` | POOL_INIT_CODE_HASH Sushi V3 Arbitrum = `0xe34f199b19b2b4f47f68442619d555527d244f78a3297ea89325f843f87b8b54` (sushi sushiswap-v3.ts L4-L5, L81) = poolInitCodeHash Uniswap V3 (UR 2.0.0 L15) → код пула = Uniswap V3 (ABI Uniswap, вид v3_fork, families.rs:308) |
| dex.factory | `0x1af415a1eba07a4986a52b6f2e7de7003d82231e` | sushi sushiswap-v3.ts L43 |
| dex.tick_lens | `0x8516944e89f296eb6473d79aed1ba12088016c9e` | sushiswap-v3.ts L158 (SUSHISWAP_V3_TICK_LENS) |
| dex.quoter | `0x0524e833ccd057e4d7a296e3aaab9f7675964ce1` | sushiswap-v3.ts L196 (SUSHISWAP_V3_QUOTER) |
| dex.v3_fee_tiers (необяз.) | не задавать до проверки | уровни фабрики Sushi V3 на Arbitrum в прочитанных источниках не перечислены (T448-12) |
| dex.pool_enabled | да | исполнение — шаг V3 исполнителя (v3_fork tradable=false) |

### 2.6 KyberSwap Classic (DMM) — `kyberswap_classic_arbitrum@arbitrum` (T433-007)

| Настройка | Значение | Источник |
| --- | --- | --- |
| dex.kind | `kyber_dmm` | Sourcify 42161 `0xD9bf…6974`: full match, DMMFactory, solc 0.6.12, один файл DMMFactory.sol с DMMPool, VolumeTrendRecorder, FeeFomula, MathExt — тот же набор, что у фабрики BSC (abi_dmm.rs:12-19); getTradeInfo → (_reserve0, _reserve1, _vReserve0, _vReserve1, feeInPrecision), getPools(IERC20, IERC20), PoolCreated(token0, token1, pool, ampBps, totalPool) |
| dex.factory | `0xD9bfE9979e9CA4b2fe84bA5d4Cf963bBcB376974` (DMMFactory, Dynamic Fee) | docs.kyberswap.com Classic Contract Addresses, Arbitrum |
| dex.quoter | `0xEaE47c5D99f7B31165a7f0c5f7E0D6afA25CFd55` (DMMRouter) | там же; на Sourcify исходника нет (оговорка: сверка роутера с DMMRouter02 — при включении) |
| dex.pool_enabled | да | — |

Оговорка: на той же странице — Static Fee (KSFactory `0x1c758aF0688502e49140230F6b0EBd376d429be5`, KSRouter
`0x5649B4DD00780e99Bab7Abb4A3d581Ea1aEB23D0`) и Legacy Static Fee (KSFactory `0x51E8D106C646cA58Caf32A47812e95887C071a62`).
Их пулы под тем же id GeckoTerminal не пройдут проверку фабрики DMM — нужен код (T448-02, T448-04).

**Решение 450:** KSPool = DMMPool с постоянной комиссией (dmm-smart-contracts@e557b57; Sourcify 42161 обеих KSFactory —
match), тот же вид. У `kyberswap_classic_arbitrum@arbitrum` — dex.sub_dex = `kyberswap_classic_arbitrum-ks@arbitrum,
kyberswap_classic_arbitrum-ksl@arbitrum` (имена — предложение, решает Тигран); дочерние ключи:

| Ключ | dex.kind | dex.factory | dex.quoter | dex.pool_enabled |
| --- | --- | --- | --- | --- |
| `kyberswap_classic_arbitrum-ks@arbitrum` | `kyber_dmm` | `0x1c758aF0688502e49140230F6b0EBd376d429be5` (KSFactory Static Fee) | `0x5649B4DD00780e99Bab7Abb4A3d581Ea1aEB23D0` (KSRouter) | да |
| `kyberswap_classic_arbitrum-ksl@arbitrum` | `kyber_dmm` | `0x51E8D106C646cA58Caf32A47812e95887C071a62` (KSFactory Legacy) | `0xC3E2aED41ECdFB1ad41ED20D45377Da98D5489dD` (KSRouter Legacy) | да |

Источник адресов — docs.kyberswap.com Classic Contract Addresses (Arbitrum). Оговорка: у Legacy KSRouter ссылка
обозревателя на странице ведёт на адрес DMMRouter — проверка ответом getAmountsOut в записи (T450-08).

### 2.7 SmarDex — `smardex-arbitrum@arbitrum` (T433-051)

| Настройка | Значение | Источник |
| --- | --- | --- |
| dex.kind | `smardex` | SmarDex-Dev/smart-contracts 7ab2beb2: deployments/arbitrum/SmardexFactory.json и deployments/bsc/SmardexFactory.json — одинаковые deployedBytecode (SHA-256 строки 08b3cf62…) и solcInputHash `1a3e5ed825e3a0bcaf935206aae1ecd3`; фабрика BSC `0xA8EF6FEa…Af33` — та, по которой сделан вид (catalog/dex.rs:99). Пара создаётся фабрикой — код пары тот же |
| dex.factory | `0x41A00e3FbE7F479A99bA6822704d9c5dEB611F22` | deployments/arbitrum/SmardexFactory.json, поле address |
| dex.quoter_virtual | да | обязателен у smardex (catalog/dex.rs:197; families.rs:591) |
| dex.pool_enabled | да | — |

### 2.8 Maverick V2 — `maverick-v2-arbitrum@arbitrum` (T433-074)

| Настройка | Значение | Источник |
| --- | --- | --- |
| dex.kind | `maverick_v2` | docs.mav.xyz v2-contract-addresses: Factory, Quoter, PoolLens — одни и те же адреса на Arbitrum, Base, BNB Smart Chain, Ethereum, Scroll (то же развёртывание, что на BSC: Quoter BSC `0xb40AfdB8…` — catalog/dex.rs:186) |
| dex.factory | `0x0A7e848Aca42d879EF06507Fca0E7b33A0a63c1e` | docs.mav.xyz |
| dex.quoter | `0xb40AfdB85a07f37aE217E7D6462e609900dD8D7A` | docs.mav.xyz |
| dex.pool_lens | `0x6A9EB38DE5D349Fe751E0aDb4c0D9D391f94cc8D` | docs.mav.xyz |
| dex.tick_radius | значение Тиграна (как у BSC) | параметр бота, не DEX (catalog/dex.rs:183) |
| dex.pair_lookup_max (необяз.) | значение Тиграна | параметр бота (catalog/dex.rs:184) |
| dex.pool_enabled | да | — |

### 2.9 Camelot V4 (Algebra Integral) — `camelot-v4@arbitrum` (T433-084)

| Настройка | Значение | Источник |
| --- | --- | --- |
| dex.kind | `algebra_integral` | Sourcify 42161 `0xBefC…2f37`: AlgebraFactory, solc 0.8.20, комментарий «@dev Version: Algebra Integral 1.2.1» (AlgebraFactory.sol). Вид поддерживает 1.2.0/1.2.1 (Swap 9 полей, Burn с pluginFee — abi_algebra_integral.rs:27) |
| dex.factory | `0xBefC4b405041c5833f53412fF997ed2f697a2f37` | docs.camelot.exchange/contracts/arbitrum/one-mainnet, AMMv4 AlgebraFactory |
| dex.quoter_virtual | да | у Integral квотер только виртуальный (families.rs:543-545). Официальный Quoter V4 `0xFe24b2cDfF01B644995bc248bA8497467d688F7B` — в код не подключён |
| dex.custom_deployers (необяз.) | не задавать | официального списка деплоеров кастомных пулов Camelot в прочитанных источниках нет |
| dex.pool_enabled | да | — |

Оговорка: ABI самого пула (globalState, ticks, события) Sourcify у фабрики в первых 100 000 знаках не показал; версия 1.2.1
— из комментария фабрики. Сверка с квотером (dex.quoter_max_diff_pct) при включении.

### 2.10 LFJ Liquidity Book V2.1 — `traderjoe-v2-1-arbitrum@arbitrum` (T433-034)

| Настройка | Значение | Источник |
| --- | --- | --- |
| dex.kind | `lb_v21` | Sourcify 42161 `0x8e42…fd4e`: exact match, LBFactory (src/LBFactory.sol), solc 0.8.10 — тот же адрес и компилятор, что у фабрики BSC (abi_lb.rs:9-10) |
| dex.factory | `0x8e42f2F4101563bF679975178e880FD87d3eFd4e` | lfj-gg/joe-sdk-v2 a6f5a8c3 src/constants/v2Addrs.ts L61-L64 (LB_FACTORY_V21_ADDRESS, ARBITRUM_ONE) |
| dex.pool_enabled | да | — |

Оговорка: реализация пары (клон) на Arbitrum не прочитана — getLBPairImplementation() фабрики на своём узле и Sourcify
реализации (T448-11). Попутно для T433-015: LBFactory v2.0 Arbitrum — `0x1886D09C9Ade0c5DB822D85D21678Db67B6c2982`
(v2Addrs.ts L52-L55, LB_FACTORY_ADDRESS).

### 2.11 Balancer V2 — `balancer_arbitrum@arbitrum` (T433-014)

| Настройка | Значение | Источник |
| --- | --- | --- |
| dex.kind | `balancer_v2` | вид сделан по Balancer 20210418-vault / 20210624-stable-pool (catalog/dex.rs:191-192); на Arbitrum развёрнуты те же задачи |
| dex.vault | `0xBA12222222228d8Ba445958a75a0704d566BF2C8` | balancer-deployments v2/tasks/20210418-vault/output/arbitrum.json |
| dex.factory | `0x7dFdEF5f355096603419239CE743BfaF1120312B` (WeightedPoolFactory 20210418) | v2/deprecated/20210418-weighted-pool/output/arbitrum.json |
| dex.balancer_stable_factory (необяз.) | `0x2433477A10FC5d31B9513C638F19eE85CaED53Fd` (StablePoolFactory 20210624) | v2/deprecated/20210624-stable-pool/output/arbitrum.json |
| dex.pool_enabled | да | — |

Оговорка (нужен код): одна фабрика весовых и одна стейблсвап. Пулы других фабрик Arbitrum не пройдут isPoolFromFactory:
WeightedPool2TokensFactory `0xCF0a32Bbef8F064969F21f7e02328FB577382018` (та же задача 20210418), weighted-pool-v2
`0x8df6EfEc5547e31B0eb7d1291B511FF8a2bf987c`, v3 `0xf1665E19bc105BE4EDD3739F88315cC699cc5b65`, v4
`0xc7E5ED1054A24Ef31D827E6F86caA58B3Bc168d7`, stable-pool-v2 `0xEF44D6786b2b4d544b7850Fe67CE6381626Bf2D6`, composable
stable v1–v6 (v2/deprecated/*composable-stable-pool*/output/arbitrum.json) — T448-03.

**Решение 450:** сверка build-info (balancer-deployments@b55dc2b8): WeightedPool2Tokens (задача 20210418) — тот же расчёт
обмена, что WeightedPool v1 (onSwap L255-L310, FixedPoint и WeightedMath задачи); weighted-pool v2 / v3 / v4 —
WeightedMath._calcOutGivenIn / _calcInGivenOut тот же текст, FixedPoint.powUp с точными степенями 1/2/4 (L134-L150),
множители — только decimals (курсы rate providers в обмен не входят — только в протокольную комиссию); stable-pool-v2
— StableMath._calculateInvariant по формуле Curve с округлением вниз (L57-L120). Настройка dex.balancer_math задаёт версию
у ключа. У `balancer_arbitrum@arbitrum` — dex.sub_dex = `balancer_arbitrum-w2t@arbitrum, balancer_arbitrum-wv2@arbitrum,
balancer_arbitrum-wv3@arbitrum, balancer_arbitrum-wv4@arbitrum` (имена — предложение); у каждого dex.kind = `balancer_v2`,
dex.vault = `0xBA12222222228d8Ba445958a75a0704d566BF2C8`, dex.pool_enabled = да и:

| Ключ | dex.factory | dex.balancer_stable_factory | dex.balancer_math |
| --- | --- | --- | --- |
| `balancer_arbitrum@arbitrum` (как выше) | `0x7dFdEF5f…312B` (20210418) | `0x2433477A…53Fd` (20210624) | не задавать (v1) |
| `balancer_arbitrum-w2t@arbitrum` | `0xCF0a32Bbef8F064969F21f7e02328FB577382018` (WeightedPool2TokensFactory, 20210418) | — | не задавать (v1) |
| `balancer_arbitrum-wv2@arbitrum` | `0x8df6EfEc5547e31B0eb7d1291B511FF8a2bf987c` (20220908-weighted-pool-v2) | `0xEF44D6786b2b4d544b7850Fe67CE6381626Bf2D6` (20220609-stable-pool-v2) | `v2` |
| `balancer_arbitrum-wv3@arbitrum` | `0xf1665E19bc105BE4EDD3739F88315cC699cc5b65` (20230206-weighted-pool-v3) | — | `v2` |
| `balancer_arbitrum-wv4@arbitrum` | `0xc7E5ED1054A24Ef31D827E6F86caA58B3Bc168d7` (20230320-weighted-pool-v4) | — | `v2` |

Источник адресов — balancer-deployments b55dc2b8 addresses/arbitrum.json. Не сделано (нужен код): composable stable
v1–v6, meta-stable 20210727, LBP 20210721, investment 20210907, managed, linear (aave / erc4626 / yearn / unbutton),
stable phantom — T450-03…07.

### 2.12 Curve — `curve_arbitrum@arbitrum` (T433-005), часть StableSwap-NG

| Настройка | Значение | Источник |
| --- | --- | --- |
| dex.kind | `curve_stableswap_ng` | curvefi/stableswap-ng 3332cd44 — тот же репозиторий, по которому сделан вид (решение 275) |
| dex.factory | `0x9AF14D26075f142eb3F292D5065EB3faa646167b` | scripts/deployments.py L24-L32 («arbitrum:mainnet» factory); scripts/deploy_pool.py L21 |
| dex.curve_ng_versions | `v7.0.0` | версия, с которой сверена математика (catalog/dex.rs:206; CurveStableSwapNG.vy L216). Реализация plain_amm Arbitrum `0xf6841C27fe35ED7069189aFD5b81513578AFD7FF` (deployments.py L29; добавлена коммитом 0b308e0 22.06.2024) — на Sourcify нет; её version() бот сверит сам, иной версии пул не возьмёт (T448-14) |
| dex.multi_pair (необяз.) | решение Тиграна | как у BSC (catalog/dex.rs:175) |
| dex.pool_enabled | да | — |

Оговорка (нужен код): под одним id GeckoTerminal у Curve — несколько семейств (одна dex.kind на ключ, families.rs:455):
2pool `0x7f90122BF0700F9E7e1F688fe926940E8839F353` (set_up_base_pools.py L76; шаблон не сверен с curve_stableswap),
Twocrypto/Tricrypto (T433-F3) — T448-02.

**Решение 450:** механизм дочерних ключей есть (dex.sub_dex у `curve_arbitrum@arbitrum`). 2pool — Vyper 0.2.12 (Sourcify
42161 match): A_PRECISION = 100, A_precise(), RATE_MULTIPLIER, двухмонетная формула D — не совпадает с шаблоном вида
curve_stableswap (Curve 2020 без A_PRECISION) и NG — нужен новый шаблон (T450-02). Crypto-пулы — дочерними ключами вида
T433-F3, когда он будет в коде.

**Решение 472:** вид `curve_stableswap_plain` есть. Дочерний ключ dex.sub_dex у `curve_arbitrum@arbitrum` (имя —
решение Тиграна, например `curve_arbitrum-plain@arbitrum`):

| Настройка | Значение | Источник |
| --- | --- | --- |
| dex.kind | `curve_stableswap_plain` | решение 472 |
| dex.stable_pools | `["0x7f90122BF0700F9E7e1F688fe926940E8839F353"]` | curvefi/curve-js ab3f1c2 src/constants/pools/arbitrum.ts L15-L20 (2pool) |
| dex.factory | `0xb17b674D9c5CB2e441F8e196a2f048A81355d031` | curve-js network_constants.ts L157 (ALIASES_ARBITRUM.factory); Sourcify 42161 match (FactorySidechains.vy, Vyper 0.2.16) |
| dex.curve_plain_implementations | `["0x54e8A25d0Ac0E4945b697C80b8372445FEA17A62", "0x73Ec37618683C274D0bBf5f5726aA856B2BDAB81", "0xD68970e266cE1A015953897C7055a5E0bC657Af8", "0x7DA64233Fefb352f8F501B357c018158ED8aA455", "0x6F9fb833501f46CBE6f6A4b6Cf32C834E5A5e8C5", "0x0100fBf414071977B19fC38e6fc7c32FE444F5C9", "0xe381C25de995d62b453aF8B931aAc84fcCaa7A62", "0xc379bA7b8e1c6C48D64e1cf9dD602C97c9fD0F40", "0xAAe75FAebCae43b9d541Fd875622BE48D9B4f5D0", "0x8866414733F22295b7563f9C5299715D2D76CAf4", "0x8d53E5De033078367Ad91527c53abfd1Eb6bfa86", "0x2ac56cEBc2D27c9bB51a11773355E44371Eb88D3", "0x89287c32c2CAC1C76227F6d300B2DBbab6b75C08", "0x06e3C4da96fd076b97b7ca3Ae23527314b6140dF"]` | curve-js factory/stable.ts L168-L183 (Plain2/3/4 Basic, Balances, ETH, Optimized; Plain2BasicEMA, Plain2ETHEMA) |
| dex.pool_enabled | после записи T472-01 — Тигран | — |

Решение 472 (доделка): реализации Balances / ETH / Optimized / EMA — тем же ключом (список выше); fraxbp 0xC9B8…40d5 —
пул фабрики на Plain2Basic (Curve API getPools/arbitrum/factory: implementationAddress 0x54e8…7A62) — берётся фабрикой.
Пулы с нативным ETH (Plain*ETH) — граф и котировка; исполнения нет (шагу CURVE исполнителя нужен вход/выход ETH —
новый шаг, TASKS T472-08).

Метапулы фабрики — второй дочерний ключ (имя — решение Тиграна, например `curve_arbitrum-meta@arbitrum`):

| Настройка | Значение | Источник |
| --- | --- | --- |
| dex.kind | `curve_stableswap_meta` | решение 472 |
| dex.factory | `0xb17b674D9c5CB2e441F8e196a2f048A81355d031` | curve-js network_constants.ts L157 |
| dex.curve_plain_implementations | `["0x09672362833d8f703D5395ef3252D4Bfa51c15ca", "0xBE175115BF33E12348ff77CcfEE4726866A0Fbd5", "0x8DEb66a4A40E370355bEe35f12E55Fe9c755d686", "0x3edE9b145F82e9e46C03f8A8F67B77aEE847b632"]` | curve-js factory/stable.ts L159-L166 (MetaUSD, MetaUSDBalances, MetaFraxUSD, MetaFraxUSDBalances) |
| dex.pool_enabled | после записи T472-01 — Тигран | — |

**Решение 472 (T472-10):** crypto-пул основного реестра EURS/2CRV `0xA827a652Ead76c6B0b3D19dba05452E06e25c27e` — вид
`curve_crypto2_v1` (вариант CryptoPair протокола tricrypto 2021: пул из списка, две монеты, математика CurveCryptoSwap2).
Третий дочерний ключ (имя — решение Тиграна, например `curve_arbitrum-eurs@arbitrum`):

| Настройка | Значение | Источник |
| --- | --- | --- |
| dex.kind | `curve_crypto2_v1` | решение 472 |
| dex.stable_pools | `["0xA827a652Ead76c6B0b3D19dba05452E06e25c27e"]` | curvefi/curve-crypto-contract 41e6289 «deployment-logs/2021-10-30. EURS/deployment.log» |
| dex.pool_enabled | после записи (tricrypto_v1.py/curve_crypto.py — T472-10) — Тигран | — |

WBTC/renBTC `0x3E01…0Fb` и ETH/wstETH `0x6eB2…7d80` — исходник не найден (TASKS T472-10), не подключены.

### 2.13 DODO V2 — три ключа (T433-086)

Пулы DODO индексатор GeckoTerminal в сети arbitrum не отдаёт (dex-coverage-arbitrum.md, строка 86): источник — поиск по
фабрике (index.factory_scan = да у arb_dex; factory_calls.rs:90 — Q::Dodo).

| Ключ (предложение) | dex.kind | dex.factory | Источник адреса | Исходник |
| --- | --- | --- | --- | --- |
| `dodo-amm-dvm@arbitrum` | `dodo_dvm` | `0xDa4c4411c55B0785e501332354A036c04833B72b` | docs.dodoex.io contracts-address/arbitrum, DVMFactory | Sourcify 42161: exact match DVMFactory, solc 0.6.9 |
| `dodo-amm-dpp@arbitrum` | `dodo_dpp` | `0xa6Cf3d163358aF376ec5e8B7Cc5e102a05FdE63D` | там же, DPPFactory | Sourcify: exact match DPPFactory, solc 0.6.9 |
| `dodo-amm-dsp@arbitrum` | `dodo_dsp` | `0xC8fE2440744dcd733246a4dB14093664DEFD5A53` | там же, DSPFactory | Sourcify: exact match DSPFactory, solc 0.6.9 |

dex.pool_enabled = да у каждого. Оговорка: шаблоны пулов (клоны) фабрик на Arbitrum не прочитаны — T448-13.

## 3. Нужен код (решение 450: сделан)

### 3.1 iZiSwap — `iziswap@arbitrum` (T433-098) — код есть, сверка — T450-01 (запись) (решение 450)

Значения есть: dex.kind `iziswap`, dex.factory `0xCFD8A067e1fa03474e79Be646c5f6b6A27847399` (izumi-docs
deployed_contracts/mainnet.rst L53-L64, «Arbitrum (chainId: 42161) (updated on 2023.07.17)», Core factory), dex.quoter_virtual
= да (обязателен, catalog/dex.rs:197), dex.pmm_ladder_usd — значение Тиграна. **Почему нужен код:** iZiSwap нет в
GeckoTerminal arbitrum, а поиск по фабрике вид iziswap не умеет (factory_calls.rs:72-103 — `_ => continue`): пулы не
появятся в справочнике. Задача T448-05. Модули ядра Arbitrum (swapX2YModule `0xAC9788cf…`, swapY2XModule `0x93C22Fbe…`)
отличаются адресами от BSC — код пула сверить тогда же.

**Решение 450:** поиск по фабрике — pool(монета, встречный, fee) по уровням dex.v3_fee_tiers (factory_ask.rs Q::Izi).
Фабрика Arbitrum — Sourcify 42161 (iZiSwapFactory, solc 0.8.4): fee2pointDelta конструктора 100 / 400 / 2000 / 10000,
реестр pool пишется в обе стороны — как iZiSwap-core@3fe21eba. Настройки: dex.kind `iziswap`, dex.factory (выше),
dex.quoter_virtual = да, dex.pmm_ladder_usd — Тигран, dex.v3_fee_tiers = `100, 400, 2000, 10000` (проверка
fee2pointDelta своим узлом — запись T450-01), dex.pool_enabled = да; у arb_dex — index.factory_scan = да.

## 4. Не подтверждено

### 4.1 PancakeSwap V2 / V3 — `pancakeswap-v2-arbitrum@arbitrum`, `pancakeswap-v3-arbitrum@arbitrum` (T433-049, T433-050)

Адреса (pancake-developer d620e1d0):

| Настройка | V2 | V3 | Источник |
| --- | --- | --- | --- |
| dex.kind | `pancakeswap_v2` | `pancakeswap_v3` | — |
| dex.factory | `0x02a84c1b3BBD7401a5f7fa98a384EBC70bB5749E` | `0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865` | v2/addresses.md L17; v3/addresses.md (Core, столбец «BSC, ETH, ARB…») |
| dex.router (V2) | `0x8cFe327CEc66d1C090Dd72bd0FF11d690C33a2Eb` | — | v2/addresses.md L39 |
| dex.quoter / dex.tick_lens (V3) | — | QuoterV2 `0xB048Bbc1Ee6b733FFfCFb9e9CeF7375518e25997`, TickLens `0x9a489505a00cE272eAa5e07Dba6491314CaE3796` | v3/addresses.md (Periphery, «BSC, ETH, Arbitrum…») |
| dex.v2_fee_source / pct | `fixed` / `0.25` | — | Sourcify 42161 `0x8cFe…a2Eb`: exact match PancakeRouter, getAmountOut `amountIn.mul(9975)` / `reserveIn.mul(10000)`; init code hash пары `0x57224589…4d2d` (= sushi pancakeswap-v2.ts L52, отличается от BSC `0x00fb7f63…`) |
| dex.v3_fee_tiers (необяз.) | — | `100, 500, 2500, 10000` | pancake-v3-contracts 98684794 PancakeV3Factory.sol L41-L53 (конструктор) |
| dex.universal_router | `0xFE6508f0015C778Bdcc1fB5465bA5ebE224C9912` | то же | universal-router/addresses.md L37 — раздел «PancakeSwap v3 Universal Router»; Infinity UR на Arbitrum нет (там же L10-L20: только BSC, Base, Robinhood) |
| dex.permit2 | `0x31c2F6fcFf4F8759b3Bd5Bf0e1084A055615c768` | то же | permit2/addresses.md L9 |
| dex.universal_router_version | `pancakeswap_v3` (решение 450) | то же | см. ниже |

Почему не подтверждено: у BSC эти DEX исполняет Infinity UR (`pancakeswap_infinity`). На Arbitrum — v3 UR. Sourcify
42161 `0xFE65…9912`: exact match UniversalRouter, solc 0.8.17; Commands.sol: V3_SWAP_EXACT_IN 0x00, V2_SWAP_EXACT_IN
0x08, WRAP_ETH 0x0b, UNWRAP_WETH 0x0c, BALANCE_CHECK_ERC20 0x0e; Dispatcher.sol: входы V2/V3 — (recipient, amountIn,
amountOutMin, path, payerIsUser) — те же байты, что пишет код для `pancakeswap_infinity` и `2.0`
(exec/swap_v2v3.rs:40 — отличие только у 2.1.1+). Не прочитаны: вход BALANCE_CHECK_ERC20 и параметры конструктора
(permit2, v2Factory, v3Deployer, v2InitCodeHash, v3InitCodeHash — RouterParameters). Вариант `pancakeswap_infinity`
включает умный роутер Infinity (dex_rules.rs:16-27), `2.0` — имя Uniswap. Нужен вариант формата для v3 UR PancakeSwap
или решение Тиграна (T448-01).

**Решение 450 — код есть, сверка — T450-01 (запись):** вариант `pancakeswap_v3` (exec/ur.rs Format::PancakeV3). Прочитано (Sourcify 42161 exact_match):
BALANCE_CHECK_ERC20 — abi.decode(address owner, address token, uint256 minBalance), успех — balanceOf(owner) ≥ minBalance;
WRAP_ETH / UNWRAP_WETH — (address, uint256); Constants: CONTRACT_BALANCE = 1 << 255, ADDRESS_THIS = address(2);
Payments.pay — CONTRACT_BALANCE = весь баланс роутера; 0x10 = OWNER_CHECK_721 (Infinity нет). Параметры конструктора
(RouterParameters): permit2 `0x31c2F6fc…c768`, weth9 `0x82aF4944…Bab1`, v2Factory `0x02a84c1b…749E`, v3Factory
`0x0BFbCF9f…1865`, v3Deployer `0x41ff9AA7…71c9` (= PancakeV3PoolDeployer, pancake-developer v3/addresses.md L12),
v2InitCodeHash `0x57224589…4d2d`, v3InitCodeHash `0x6ce8eb47…f7e2`. Пулы Infinity этим форматом не исполняются.

### 4.2 PancakeSwap StableSwap — `pancakeswap-stableswap-arbitrum@arbitrum` (T433-064)

dex.kind `pancake_stableswap`, нужна dex.factory. Официальная страница pancake-developer
docs/pages/contracts/stableswap/overview.md L9-L12 даёт фабрику только для BNB Smart Chain; адреса Arbitrum в
официальных источниках этой проверки нет. Исходник развёртывания не сверен (T448-10).

**Решение 451 (проверка 20.11.2026).** Что найдено:
- StableSwap на Arbitrum есть: @pancakeswap/stable-swap-sdk 2.0.16 (npm, sha256 tgz 6f210b2e…5faa; src/constants/pools/
  pools.ts — STABLE_SUPPORTED_CHAIN_IDS включает ARBITRUM_ONE), pancakeswap/pancakeswap-ai f8d2f9d
  packages/plugins/pancakeswap-driver/skills/liquidity-planner/SKILL.md L71, L540 («StableSwap (BSC, Ethereum and
  Arbitrum)»).
- Список пулов SDK берёт с API `https://configs.pancakeswap.com/api/data/cached/stableswaps?chainId=42161`
  (src/config/endpoint.ts, src/constants/pools/index.ts) — из песочницы закрыт (прокси 403, WebFetch не разрешён).
- Адреса фабрики нет: @pancakeswap/smart-router 7.8.1 (sha256 tgz 24ad6ab1…c71a) — STABLE_SWAP_INFO_ADDRESS
  [ARBITRUM_ONE] = "" (dist/chunk-3T32IMO3.mjs); SmartRouter Arbitrum `0x32226588378236Fd0c7c4053999F88aC0e5cAc77`
  (Sourcify 42161 match, contracts/SmartRouter.sol, solc 0.7.6) — constructorArguments: _factoryV2 0x02a8…749e,
  _deployer 0x41ff…71c9, _factoryV3 0x0bfb…1865, _positionManager 0x46a1…4364, **_stableFactory = 0, _stableInfo = 0**,
  WETH9 0x82af…bab1 (порядок — swapRouter02Abi конструктора в smart-router dist/evm.mjs L950-L958); v3 Universal Router
  `0xFE65…9912` (Sourcify) — RouterParameters (contracts/base/RouterImmutables.sol) stableFactory = 0, stableInfo = 0
  (слова конструктора по выдаче Sourcify; сверить своим узлом: SmartRouter.stableSwapFactory()).
Итог: официального адреса фабрики StableSwap Arbitrum в прочитанных источниках нет; API конфигов не прочитан.
Вопрос Тиграну (T448-10): получить ответ API конфигов со своей машины (stableSwapAddress, lpAddress, infoStableSwapAddress
пулов chainId=42161); без адреса фабрики вид pancake_stableswap не подключается (families.rs: нужна dex.factory).

### 4.3 KyberSwap Elastic — `kyberswap_elastic_arbitrum@arbitrum` (T433-006)

Официальный репозиторий KyberNetwork/ks-elastic-sc 4ab08c0a README.md L6-L17 — «Deployed contracts for KyberSwap
Elastic v1» (в т.ч. Arbitrum): Factory `0x5F1dddbf348aC2fbe22a163e30F99F9ECE3DD50a`, Router
`0xC1e7dFE73E1598E3910EF4C7845B68A9Ab6F4c83`, QuoterV2 `0x0D125c15D54cA1F8a813C74A81aEe34ebB508C1f`, TicksFeesReader
`0x165c68077ac06c83800d19200e6E2B08D02dE75D`. Sourcify 42161: все четыре — full match, solc 0.8.9; у фабрики
getPool(address, address, uint24), PoolCreated(token0, token1, swapFeeUnits, tickDistance, pool), getPoolState → (sqrtP,
currentTick, nearestCurrentTick, locked), getLiquidityState → (baseL, reinvestL, reinvestLLast) — как в abi_elastic.rs.
Почему не подтверждено: вид сделан по коду HEAD 4ab08c0 («KyberSwap Elastic v2», README L1), развёрнуто — v1; математика
обмена v1 и v2 не сравнивалась. Страница адресов Elastic на docs.kyberswap.com больше не открывается (страница «не
найдена», в sitemap.md нет Elastic). Задача T448-06.

**Решение 451 — сравнение v1 (Sourcify 42161, исходник фабрики 0x5F1d…D50a: Pool.sol, PoolStorage.sol, PoolTicksState.sol,
libraries/*) с видом (ks-elastic-sc 4ab08c0, первый коммит v2 0e7713a).** Отличие в математике обмена одно —
SwapMath.computeSwapStep: v1 `(isExactInput && usedAmount >= specifiedAmount)`, v2 `>` (4ab08c0 SwapMath.sol L60; в
0e7713a ещё `>=`). Совпадают: calcReachAmount, estimateIncrementalLiquidity, calcIncrementalLiquidity, calcFinalPrice,
calcReturnedAmount, QuadMath (getSmallerRootOfQuadEqn, sqrt), FullMath.mulDivCeiling, MAX_TICK_DISTANCE = 480, цикл
Pool.swap (tempNextTick, targetSqrtP, liquidity = baseL + reinvestL, пересечение тика), _getInitialSwapData,
_updateLiquidityAndCrossTick, раскладка PoolStorage (ticks 0, initializedTicks 1, positions 2, poolData 3…), события
Swap/Mint/Burn/BurnRTokens, getPoolState/getLiquidityState/ticks/initializedTicks, ISwapCallback.swapCallback, QuoterV2
`0x0D12…8C1f` (QuoteExactInputSingleParams / QuoteOutput — как IElasticQuoterV2). Прочие отличия v1 на расчёт обмена не
влияют: нет startSqrtP (тик после неполного шага пересчитывается всегда — наш цикл тик после останова не выдаёт),
нет PoolOracle, MIN_LIQUIDITY 100000. TicksFeesReader v1 `0x165c…E75D`: getTicksInRange(length = 0) считает длину массива
иначе — `(MAX_TICK − startTick) / tickDistance` (+1 при startTick 0 / MAX_TICK), у v2 — по MIN_TICK отдельной формулой.
Сделано: вариант вида **`kyber_elastic_v1`** (pools/family_variant.rs, math/swap_elastic_step.rs compute_swap_step_rule).

| Настройка | Значение | Источник |
| --- | --- | --- |
| dex.kind | `kyber_elastic_v1` | решение 451 |
| dex.factory | `0x5F1dddbf348aC2fbe22a163e30F99F9ECE3DD50a` | ks-elastic-sc README.md L6-L17 (v1), Sourcify full match |
| dex.router | `0xC1e7dFE73E1598E3910EF4C7845B68A9Ab6F4c83` | то же |
| dex.quoter | `0x0D125c15D54cA1F8a813C74A81aEe34ebB508C1f` | то же |
| dex.tick_reader | `0x165c68077ac06c83800d19200e6E2B08D02dE75D` | то же |
| dex.pool_enabled | да — решает Тигран | — |

Оговорки: (1) запись своего узла (tools/arbitrum_record/kyber_elastic_v1.py) и тест tests/arbitrum_kyber_elastic_v1_live.rs —
наш расчёт = QuoterV2; (2) getTicksInRange(pool, MIN_TICK, 0) выделяет массив на все возможные тики: у пула с
tickDistance 1 это ≈1,77 млн слов (расчёт по TicksFeesReader.sol); уложится ли такой вызов в предел газа eth_call узла —
покажет запись (скрипт пишет ответ или ошибку; относится и к BSC, вид не менялся); (3) вид исполнения не имеет (tradable = false, как у
kyber_elastic) — только сигнал; (4) включать ли Elastic — решает Тигран (вопрос T448-06; страницы Elastic в документации
docs.kyberswap.com нет — решение 448).

### 4.4 Aero Slipstream — `aero-slipstream-arbitrum@arbitrum` (T433-083)

dex.kind `slipstream` требует dex.factory, dex.quoter (+ dex.cl_tick_spacings для поиска). В официальных репозиториях
aerodrome-finance/slipstream (f8717faa) и velodrome-finance/slipstream (193a6b85) развёртываний Arbitrum нет
(script/constants/output: только Optimism и Base). Адресов из официального источника нет, ABI не сверен (T448-09).

**Решение 451.** Aero (слияние Aerodrome и Velodrome, запуск 21.10.2026, в т.ч. Arbitrum — aero.xyz/aero-upgrade;
организация GitHub со страницы — Dromos-Labs). Официальный исходник и адреса: **Dromos-Labs/metadex-slipstream-public**
`f99f209` — VERSIONS: версия 1.1.0, коммит `cf1ae46f` («export deployed Slipstream V3 source»), развёртывание arbitrum —
deployment-addresses/cl-arbitrum.json: clFactory `0x88a60ee77392577c355C0acb2A8d476D5a4EC03a`, clPoolImplementation
`0x1642634C8CB1C9f1B84c9a1a08534F8c3F4E89f8`, swapRouter `0x9Ee35Bf1bd69a03c6F0393582030fCe8Bb93D418`,
nftPositionManager `0x4857DB576eE0e45F10525eC59527BfD10beeC60b`, dynamicSwapFeeHook `0xC0ed888AfE87ed1AcF01D9f19bB850159e3ce434`,
clMevTaxModule `0xce015007D03f3E3964938f11c29F24bDB0b945aB`, metaquoter `0x1dDfeC5058180C207F55BB77D95AcDd0d8227868`
(QuoterV2 в файле нет); крючок — Dromos-Labs/metadex-public `2ce1816` V3/src/hooks/dynamic/DynamicSwapFeeHook.sol,
ArbDynamicSwapFeeHook.sol (VERSIONS 1.1.0, коммит 79c8d5b6). Лицензия исходников — Dromos Restricted Use License 1.0.
Сверка с видом slipstream: совпадают slot0 (6 полей, CLPool.sol L55-L71), ticks() (Tick.Info 10 полей, Tick.sol
L17-L44), getPool(a, b, int24) и PoolCreated (ICLFactory.sol L103, L215), Swap (ICLPoolEvents.sol L72-L80), цикл обмена V3
(CLPool.sol L567-L670). **Не совпадает комиссия:** state.fee = крючок beforeSwap (CLPool.sol L543-L556; SwapHookLib.sol
beforeSwap — крючок или tickSpacingToFee), у DynamicSwapFeeHook она зависит от блока (первый обмен в блоке задаёт
комиссию блока по TWAP-тику, blockFee[pool][номер блока Arbitrum]), от вызывающего (скидка DiscountRegistry), плюс MEV-налог
mevTaxModule.mevTax(); после цикла afterSwap может добавить комиссию к amountCalculated (SwapHookLib.sol afterSwap).
fee() пула — «legacy»-представление с пустыми параметрами (CLPool.sol L189-L195), не комиссия конкретного обмена.
Итог: **нужен код — новое семейство «Slipstream V3 (Aero)»** (комиссия крючка, состояние крючка и модуля MEV, квотер
Metaquoter), вид slipstream с dex.factory = clFactory котировал бы неверно. Задача T448-09.

### 4.5 Saddle — `saddle-finance@arbitrum` (T433-087)

dex.kind `saddle_stableswap`, dex.stable_pools — плоские пулы saddle-contract 2cfdd055 deployments/arbitrum_mainnet:
SaddleArbUSDPool `0xBea9F78090bDB9e662d8CB301A00ad09A5b756e9`, SaddleArbUSDPoolV2 `0xfeEa4D1BacB0519E8f952460A70719944fe56Ee0`,
SaddleFRAXBPPool `0x401AFbc31ad2A3Bc0eD8960d63eFcDEA749b4849`, CommunityfUSDCPool `0x773f9144969FA19CdB4cAE1ef19D7A381f5F6922`
(мета-пулы MetaSwap — другой контракт, не брать). В ABI всех четырёх есть getToken, getTokenBalance, getAPrecise,
swapStorage, calculateSwap, swap, paused (как в виде). Почему не подтверждено: вид сделан по Nerve (nerve-finance/contracts
bda7578, families.rs Proto::SaddleStable); у Saddle V2-пулов библиотеки SwapUtils + AmplificationUtils другой версии —
математика не сравнивалась (T448-07). Пулы — только списком (решение 292), индексатор их не отдаёт.

**Решение 451.** Пулы deployments/arbitrum_mainnet связаны с библиотеками SwapUtils `0xcF70E2F3567ba396F3dd04822F78Ecd70Ba46894`
и AmplificationUtils `0x4f6A43Ad7cba042606dECaCA730d4CE0A57ac62e` (solcInputs/83663b3e…; тексты SwapUtils.sol,
AmplificationUtils.sol, MathUtils.sol = contracts/*.sol коммита 2cfdd055). Математика совпадает с Nerve: getD L307-L357,
getY L428-L480, _calculateSwap L519-L540 (dy − dyFee)/multiplier, MAX_LOOP_LIMIT 256 (L124), A_PRECISION 100
(AmplificationUtils.sol L25), _getAPrecise (рампа, L60-L88), within1. Отличия ABI (исправлены в виде saddle_stableswap,
поведение на BSC/Nerve то же): swapStorage() — 7 полей, а не 10 (ABI deployments/arbitrum_mainnet/*.json; разбор по длине
ответа — abi_saddle::swap_fee), событие FlashLoan у SwapFlashLoan (меняет balances, SwapFlashLoan.sol L98-L141) —
перечитать пул. Swap.sol: только `payable` (c686…/e454… против 83663b…), PermissionlessSwap — свой withdrawAdminFees.
Настройки: dex.kind `saddle_stableswap`, dex.stable_pools — четыре пула выше. Оговорка — запись tools/arbitrum_record/saddle.py
(тест tests/arbitrum_saddle_live.rs: 7 слов swapStorage, наш выход = calculateSwap). Шаг исполнителя — ExecKind::Saddle
(swap(uint8,uint8,uint256,uint256,uint256) — тот же; у Saddle swap дополнительно `dx <= balanceOf(msg.sender)` и вход по
разнице баланса, SwapUtils.sol L672-L691).

### 4.6 WOOFi — `woofi-swap@arbitrum` (T433-095)

dex.kind `woofi_v2`; dex.pmm_pool `0x5520385bFcf07Ec87C4c53A7d8d65595Dff69FA4` (WooPPV2.2) и dex.oracle
`0xCf4EA1688bc23DD93D933edA535F8B72FC8934Ec` (WooracleV2_2_1) — woonetwork/docs 9bcc162d references/readme/arbitrum-one.md
L5. Тот же адрес WooPPV2.2, что на BSC (bnb-smart-chain.md). Почему не подтверждено: (1) оракул Arbitrum — WooracleV2_2_1,
вид читает state() по IWooracleV2_2 (abi_woo.rs:11-14), в WooPoolV2 f73a0f64 файла WooracleV2_2_1 нет — интерфейс не
сверен; (2) dex.pool_implementation — адрес реализации из слота EIP-1967 прокси читается только своим узлом (arbiscan и RPC
из песочницы закрыты). Задача T448-08. Пулы — поиском по фабрике (Q::Woo) и dex.multi_pair.

**Решение 451.** (1) Оракул `0xCf4E…34Ec`: Sourcify 42161 exact match — контракт **WooracleV2_2**
(contracts/wooracle/WooracleV2_2.sol, solc 0.8.14; та же раскладка, что woonetwork/WooPoolV2 f73a0f64
contracts/wooracle/WooracleV2_2.sol: TokenInfo, CLOracle, PriceRange, state(address) L325, price(address) L272);
State {uint128 price; uint64 spread; uint64 coeff; bool woFeasible} — IWooracleV2_2.sol L40-L45 = abi_woo::IWooracle;
документация woonetwork/docs resources/on-chain-price-feeds.md («Wooracle v2.2.1», Arbitrum — этот адрес, state()) —
чтение вида подходит, код не менялся. (2) Прокси `0x5520…9FA4`: Sourcify 42161 exact match TransparentUpgradeableProxy
(блок 317548902), proxyResolution → реализация `0x7b5fDb8C89A616d319202704B0eA18b168654281` — тот же адрес, что
dex.pool_implementation на BSC (docs/OWN_NODE_SWITCH.md); исходник реализации на Sourcify не найден (как на BSC —
решение 122 F, 3: своей математики нет, котировка — квотером контракта). Настройки: dex.kind `woofi_v2`, dex.pmm_pool
`0x5520385bFcf07Ec87C4c53A7d8d65595Dff69FA4`, dex.oracle `0xCf4EA1688bc23DD93D933edA535F8B72FC8934Ec`,
dex.pool_implementation `0x7b5fDb8C89A616d319202704B0eA18b168654281`; у площадки index.factory_scan = да. Оговорка —
слот EIP-1967 и селекторы в коде реализации своим узлом (tools/arbitrum_record/woofi.py, tests/arbitrum_woofi_live.rs);
вид при другом адресе в слоте пары не берёт (verify_woo.rs).

## 5. Что не делалось

- Значения on-chain (eth_call, слоты) — не читались: узла Arbitrum из песочницы нет. Всё, что ими проверяется, — в
  задачах T448-*.
- Настройки площадки arb_dex (область V) — вне этой задачи.
- dex.pool_enabled ни у одной DEX не ставился — настройки задаёт Тигран в интерфейсе (CLAUDE.md, «Стек»).
