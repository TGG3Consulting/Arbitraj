> **ЖЕЛЕЗОБЕТОННОЕ ПРАВИЛО ТИГРАНА (20.11.2026):** «Не пропустить ни единого DEX. Все DEX, которые есть, — без разделения на мелкие/крупные, без «можно потом». Ты ничего не решаешь в плане приоритетов.»

# Покрытие DEX сети Arbitrum One — решение 433 (19.11.2026)

Источники (только факты):
- **GeckoTerminal** `GET /api/v2/networks/arbitrum/dexes` — записано со своего сервера 19.11.2026:
  `tests/data/arbitrum_node_2026-11-19/geckoterminal/dexes_page1.json` (одна страница, `last` = page 1, **84 DEX**).
- **DefiLlama** `GET https://api.llama.fi/overview/dexs/arbitrum` — прочитано 19.11.2026 (WebFetch, ответ 232 126 знаков,
  **160 записей**): категории «Dexs» и «DEX Aggregator» — 107 записей (таблица 2), остальные 53 — другие
  категории (перечислены в конце, не DEX). Ответ не записан в tests/data — запись с сервера: задача T433-R1.
- **Код** — `src/venues/bsc_dex/pools/families.rs` (вид пула = dex.kind, решение 58), номера строк — на коммите ветки t217.
- **Официальные адреса** — только со страниц документации DEX, прочитанных в этой проверке (Uniswap V3, Camelot V2/V3/V4,
  PancakeSwap V3); остальные — «не сверены» (задача в каждой строке).
- Семейства по исследованию официальной документации 20.11 — `docs/notes/arbitrum/docs-check-2026-11-20.md`.

Статусы: **вид есть в коде** — модуль вида пула есть, но ABI развёртывания в Arbitrum и адреса не сверены (BSC-код по
ним не проверялся); **семейства нет в коде**; **не пул** (RFQ, книга ордеров, оракул, агрегатор); **семейство не
установлено** — в этой проверке нет официального источника, какое у DEX семейство пулов. «Вид есть в коде» ≠ «подключено»:
у Arbitrum нет ни одной DEX, подключённой в настройках (площадка arb_dex ещё не поднята — step5-plan.md).

Итог по 84 DEX GeckoTerminal: вид есть в коде — 16 (решение 440: + Fluid), частично — 1, семейства нет — 6,
семейство не установлено — 61, не пул — 0. Из DefiLlama сверх GeckoTerminal — 62 DEX.
**Решение 448 (20.11.2026):** Sushi V3 (строка 44) — семейство установлено (v3_fork): GeckoTerminal — вид есть в коде 16,
семейство не установлено 60. Настройки области «DEX» arb_dex для всех DEX с видом в коде — docs/notes/arbitrum/dex-settings-arbitrum.md:
из 20 (15 GeckoTerminal + Curve частично + 4 DefiLlama) готово 12, нужен код 1 (iZiSwap), не подтверждено 7;
плюс Sushi V3 — готово. Статус каждой строки — в столбце «Статус» (→ …).
**У каждой строки — задача с номером (docs/TASKS.md, «Решение 433»). Приоритетов нет: все задачи обязательны.**

## 1. GeckoTerminal, сеть arbitrum — все 84 DEX (порядок ответа API)

| № | id GeckoTerminal | Название | Семейство пулов (основание) | В коде (файл:строка) | Статус | Официальные адреса | Задача |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | `sushiswap_arbitrum` | SushiSwap (Arbitrum) | Sushi V2 (docs/notes/arbitrum/docs-check-2026-11-20.md) | `v2_fork` — src/venues/bsc_dex/pools/families.rs:297 | **вид есть в коде** → **готово** (решение 448) | адреса, источники, статус — docs/notes/arbitrum/dex-settings-arbitrum.md (решение 448) | T433-001 |
| 2 | `uniswap_v3_arbitrum` | Uniswap V3 (Arbitrum) | Uniswap V3 | `uniswap_v3` — src/venues/bsc_dex/pools/families.rs:292 | **вид есть в коде** → **готово** (решение 448) | адреса, источники, статус — docs/notes/arbitrum/dex-settings-arbitrum.md (решение 448) | T433-002 |
| 3 | `swapr_arbitrum` | Swapr (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-003 |
| 4 | `fraxswap_arbitrum` | Fraxswap (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-004 |
| 5 | `curve_arbitrum` | Curve (Arbitrum) | StableSwap — есть; Twocrypto/Tricrypto — нет (docs/notes/arbitrum/docs-check-2026-11-20.md) | `curve_stableswap` — src/venues/bsc_dex/pools/families.rs:341; `curve_stableswap_ng` — src/venues/bsc_dex/pools/families.rs:343 | **частично: вид есть / семейства нет** → **готово для NG (curve_stableswap_ng); 2pool и crypto — T448-02, T433-F3** (решение 448) → решение 450: дочерние ключи dex.sub_dex есть; 2pool / старые plain — **нужен код (T450-02)**; crypto — T433-F3 | адреса, источники, статус — docs/notes/arbitrum/dex-settings-arbitrum.md (решение 448) | T433-005 |
| 6 | `kyberswap_elastic_arbitrum` | Kyberswap Elastic (Arbitrum) | KyberSwap Elastic | `kyber_elastic` — src/venues/bsc_dex/pools/families.rs:316; `kyber_elastic_v1` (решение 451) | **вид есть в коде** → **готово — вариант вида `kyber_elastic_v1` (решение 451); запись узла и решение о включении — T448-06** | адреса, источники, статус — docs/notes/arbitrum/dex-settings-arbitrum.md (решение 448) | T433-006 |
| 7 | `kyberswap_classic_arbitrum` | Kyberswap Classic (Arbitrum) | KyberSwap Classic DMM | `kyber_dmm` — src/venues/bsc_dex/pools/families.rs:352 | **вид есть в коде** → **готово (только пулы DMM; Static Fee — T448-04)** (решение 448) → Static Fee + Legacy — **код есть, сверка — T450-01 (запись)** (решение 450, дочерние ключи) | адреса, источники, статус — docs/notes/arbitrum/dex-settings-arbitrum.md (решение 448) | T433-007 |
| 8 | `elk_finance_arbitrum` | Elk Finance (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-008 |
| 9 | `camelot` | Camelot | Camelot V2: направленные комиссии, стабильные пары (docs/notes/arbitrum/docs-check-2026-11-20.md) | — | **семейства нет в коде** | docs.camelot.exchange/contracts/arbitrum/one-mainnet: AMM V2 Factory 0x6EcCab422D763aC031210895C81787E87B43A652, Router 0xc873fEcbd354f5A56E00E710B90EF4201db2448d | T433-009 |
| 10 | `3xcalibur` | 3xcalibur | — | — | **семейство не установлено** | не сверены | T433-010 |
| 11 | `swapfish` | SwapFish | — | — | **семейство не установлено** | не сверены | T433-011 |
| 12 | `arbswap_arbitrum_one` | Arbswap (Arbitrum One) | — | — | **семейство не установлено** | не сверены | T433-012 |
| 13 | `oreoswap` | OreoSwap | — | — | **семейство не установлено** | не сверены | T433-013 |
| 14 | `balancer_arbitrum` | Balancer V2 (Arbitrum) | Balancer V2 | `balancer_v2` — src/venues/bsc_dex/pools/families.rs:356 | **вид есть в коде** → **готово (фабрики 20210418/20210624; остальные — T448-03)** (решение 448) → решение 450: 2Tokens, weighted v2/v3/v4, stable v2 — код есть, сверка — T450-01 (запись); composable stable, meta-stable, LBP, linear, managed — **нужен код (T450-03…07)** | адреса, источники, статус — docs/notes/arbitrum/dex-settings-arbitrum.md (решение 448) | T433-014 |
| 15 | `traderjoe-v2-arbitrum` | LFJ V2 (Arbitrum) | Liquidity Book v2.0 | (в коде только `lb_v21` — src/venues/bsc_dex/pools/families.rs:358) | **семейства нет в коде** | не сверены | T433-015 |
| 16 | `magicswap` | Magicswap | — | — | **семейство не установлено** | не сверены | T433-016 |
| 17 | `zyberswap` | Zyberswap | — | — | **семейство не установлено** | не сверены | T433-017 |
| 18 | `solidlizard` | SolidLizard | — | — | **семейство не установлено** | не сверены | T433-018 |
| 19 | `sharkyswap` | SharkySwap | — | — | **семейство не установлено** | не сверены | T433-019 |
| 20 | `alienfi` | AlienFi | — | — | **семейство не установлено** | не сверены | T433-020 |
| 21 | `arbiswap` | ArbiSwap | — | — | **семейство не установлено** | не сверены | T433-021 |
| 22 | `sterling` | Sterling | — | — | **семейство не установлено** | не сверены | T433-022 |
| 23 | `oasisswap` | OasisSwap | — | — | **семейство не установлено** | не сверены | T433-023 |
| 24 | `mindgames-arbitrum` | Mindgames (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-024 |
| 25 | `archly-arbitrum` | Archly (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-025 |
| 26 | `arbidex` | Arbidex | — | — | **семейство не установлено** | не сверены | T433-026 |
| 27 | `apeswap-arbitrum` | Apeswap (Arbitrum) | — (на BSC — V2-форк, docs/notes/dex-coverage-map.md; на Arbitrum не сверено) | — | **семейство не установлено** | не сверены | T433-027 |
| 28 | `mm-finance-arbitrum` | MM Finance (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-028 |
| 29 | `ramses` | Ramses | — | — | **семейство не установлено** | не сверены | T433-029 |
| 30 | `auragi` | Auragi | — | — | **семейство не установлено** | не сверены | T433-030 |
| 31 | `aegis` | Aegis | — | — | **семейство не установлено** | не сверены | T433-031 |
| 32 | `mm-finance-v3-arbitrum` | MM Finance V3 (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-032 |
| 33 | `camelot-v3` | Camelot V3 | Algebra 1.9 с направленными комиссиями feeZtO/feeOtZ (docs/notes/arbitrum/docs-check-2026-11-20.md) | (вид Algebra с одной комиссией — `thena_fusion` — src/venues/bsc_dex/pools/families.rs:328) | **семейства нет в коде** | docs.camelot.exchange/contracts/arbitrum/one-mainnet: AMM V3 AlgebraFactory 0x1a3c9B1d2F0529D97f2afC5136Cc23e58f1FD35B, Quoter 0x0Fc73040b26E9bC8514fA028D998E73A254Fa76E, SwapRouter 0x1F721E2E82F6676FCE4eA07A5958cF098D339e18 | T433-033 |
| 34 | `traderjoe-v2-1-arbitrum` | LFJ V2.1 (Arbitrum) | Liquidity Book v2.1 | `lb_v21` — src/venues/bsc_dex/pools/families.rs:358 | **вид есть в коде** → **готово (реализация пары — T448-11)** (решение 448) | адреса, источники, статус — docs/notes/arbitrum/dex-settings-arbitrum.md (решение 448) | T433-034 |
| 35 | `arbswap-stableswap-arbitrum` | Arbswap StableSwap (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-035 |
| 36 | `whitehole-arbitrum` | Whitehole (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-036 |
| 37 | `chronos` | Chronos | — | — | **семейство не установлено** | не сверены | T433-037 |
| 38 | `lfgswap-arbitrum` | LFGswap (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-038 |
| 39 | `swaprum` | Swaprum | — | — | **семейство не установлено** | не сверены | T433-039 |
| 40 | `solunea-arbitrum` | Solunea (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-040 |
| 41 | `arbidex-v3` | Arbidex V3 | — | — | **семейство не установлено** | не сверены | T433-041 |
| 42 | `crescentswap` | CrescentSwap | — | — | **семейство не установлено** | не сверены | T433-042 |
| 43 | `ramses-v2` | Ramses V2 | — | — | **семейство не установлено** | не сверены | T433-043 |
| 44 | `sushiswap-v3-arbitrum` | Sushiswap V3 (Arbitrum) | Sushi V3 — код пула Uniswap V3: POOL_INIT_CODE_HASH Sushi V3 Arbitrum = Uniswap V3 (решение 448) | `v3_fork` — src/venues/bsc_dex/pools/families.rs:308 | **вид есть в коде** → **готово** (решение 448) | адреса, источники, статус — docs/notes/arbitrum/dex-settings-arbitrum.md (решение 448) | T433-044 |
| 45 | `dexswap` | dexSWAP | — (на BSC DexSwap — Algebra Integral, комментарий src/venues/bsc_dex/pools/families.rs:74; на Arbitrum не сверено) | — | **семейство не установлено** | не сверены | T433-045 |
| 46 | `forge-sx` | Forge Sx | — | — | **семейство не установлено** | не сверены | T433-046 |
| 47 | `degenbrains` | Degenbrains | — | — | **семейство не установлено** | не сверены | T433-047 |
| 48 | `spartadex` | SpartaDEX | — | — | **семейство не установлено** | не сверены | T433-048 |
| 49 | `pancakeswap-v2-arbitrum` | Pancakeswap V2 (Arbitrum) | PancakeSwap V2 | `pancakeswap_v2` — src/venues/bsc_dex/pools/families.rs:290 | **вид есть в коде** → **не подтверждено — формат Universal Router v3 PancakeSwap (T448-01)** (решение 448) → **код есть, сверка — T450-01 (запись)** (решение 450: `pancakeswap_v3`) | адреса, источники, статус — docs/notes/arbitrum/dex-settings-arbitrum.md (решение 448) | T433-049 |
| 50 | `pancakeswap-v3-arbitrum` | Pancakeswap V3 (Arbitrum) | PancakeSwap V3 | `pancakeswap_v3` — src/venues/bsc_dex/pools/families.rs:291 | **вид есть в коде** → **не подтверждено — формат Universal Router v3 PancakeSwap (T448-01)** (решение 448) → **код есть, сверка — T450-01 (запись)** (решение 450: `pancakeswap_v3`) | адреса, источники, статус — docs/notes/arbitrum/dex-settings-arbitrum.md (решение 448) | T433-050 |
| 51 | `smardex-arbitrum` | SmarDex (Arbitrum) | SmarDex | `smardex` — src/venues/bsc_dex/pools/families.rs:354 | **вид есть в коде** → **готово** (решение 448) | адреса, источники, статус — docs/notes/arbitrum/dex-settings-arbitrum.md (решение 448) | T433-051 |
| 52 | `darwinswap` | DarwinSwap | — | — | **семейство не установлено** | не сверены | T433-052 |
| 53 | `pixelswap-arbitrum` | PixelSwap (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-053 |
| 54 | `antfarm-arbitrum` | Antfarm (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-054 |
| 55 | `dex-on-crypto-arbitrum` | Dex on Crypto (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-055 |
| 56 | `shekelswap` | ShekelSwap | — | — | **семейство не установлено** | не сверены | T433-056 |
| 57 | `spinaqdex` | SpinaqDex | — | — | **семейство не установлено** | не сверены | T433-057 |
| 58 | `woken-exchange-arbitrum` | Woken Exchange (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-058 |
| 59 | `kaleidoswap-arbitrum` | KaleidoSwap (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-059 |
| 60 | `gasx` | GasX | — | — | **семейство не установлено** | не сверены | T433-060 |
| 61 | `uniswap-v2-arbitrum` | Uniswap V2 (Arbitrum) | Uniswap V2 | `uniswap_v2` — src/venues/bsc_dex/pools/families.rs:290 | **вид есть в коде** → **готово** (решение 448) | адреса, источники, статус — docs/notes/arbitrum/dex-settings-arbitrum.md (решение 448) | T433-061 |
| 62 | `dackieswap-v2-arbitrum` | DackieSwap V2 (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-062 |
| 63 | `dackieswap-v3-arbitrum` | DackieSwap V3 (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-063 |
| 64 | `pancakeswap-stableswap-arbitrum` | Pancakeswap Stableswap (Arbitrum) | PancakeSwap StableSwap | `pancake_stableswap` — src/venues/bsc_dex/pools/families.rs:362 | **вид есть в коде** → **не подтверждено — нет официального адреса фабрики Arbitrum; у SmartRouter / v3 UR Arbitrum stableFactory = 0; API конфигов PancakeSwap из песочницы закрыт (T448-10, решение 451)** | адреса, источники, статус — docs/notes/arbitrum/dex-settings-arbitrum.md (решение 448) | T433-064 |
| 65 | `solidly-v3-arbitrum` | Solidly V3 (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-065 |
| 66 | `kewlswap-arbitrum` | KEWLSwap (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-066 |
| 67 | `traderjoe-v2-2-arbitrum` | LFJ V2.2 (Arbitrum) | Liquidity Book v2.2 | (в коде только `lb_v21` — src/venues/bsc_dex/pools/families.rs:358) | **семейства нет в коде** | не сверены | T433-067 |
| 68 | `moonbase-alpha` | Moonbase Alpha | — | — | **семейство не установлено** | не сверены | T433-068 |
| 69 | `deltaswap` | Deltaswap (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-069 |
| 70 | `ethervista-arbitrum` | EtherVista (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-070 |
| 71 | `squadswap-v2-arbitrum` | SquadSwap V2 (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-071 |
| 72 | `squadswap-v3-arbitrum` | SquadSwap V3 (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-072 |
| 73 | `uniswap-v4-arbitrum` | Uniswap V4 (Arbitrum) | Uniswap V4 | `uniswap_v4` — src/venues/bsc_dex/pools/families.rs:295 | **вид есть в коде** → **готово** (решение 448) | адреса, источники, статус — docs/notes/arbitrum/dex-settings-arbitrum.md (решение 448) | T433-073 |
| 74 | `maverick-v2-arbitrum` | Maverick V2 (Arbitrum) | Maverick V2 | `maverick_v2` — src/venues/bsc_dex/pools/families.rs:388 | **вид есть в коде** → **готово** (решение 448) | адреса, источники, статус — docs/notes/arbitrum/dex-settings-arbitrum.md (решение 448) | T433-074 |
| 75 | `omni-exchange-v2-arbitrum` | Omni Exchange V2 (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-075 |
| 76 | `omni-exchange-v3-arbitrum` | Omni Exchange V3 (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-076 |
| 77 | `balancer-v3-arbitrum` | Balancer V3 (Arbitrum) | Balancer V3 (docs/notes/arbitrum/docs-check-2026-11-20.md) | `balancer_v3` — src/venues/bsc_dex/pools/families.rs (Proto::BalancerV3, решение 441) | **вид есть в коде** (решения 441, 452: все пулы официальных фабрик — весовые, стейблсвап, StableSurge, Gyro 2-CLP / E-CLP, ReClamm, LBP v1–v4, LBP с фиксированной ценой, QuantAMM — и крючки StableSurge / MEV Capture; запись узла и тест — не сделаны; буфер ERC4626 — вопрос) | balancer/balancer-deployments@b55dc2b8 output/arbitrum.json: Vault 0xbA1333333333a1BA1108E8412f11850A5C319bA9, Router 0xEAedc32a51c510d35ebC11088fD5fF2b47aACF2E, фабрики — docs/FIRST_RUN.md | T433-077 |
| 78 | `fluid-arbitrum` | Fluid (Arbitrum) | Fluid DEX T1 (docs/notes/arbitrum/docs-check-2026-11-20.md) | `fluid_t1` — src/venues/bsc_dex/pools/families.rs (решение 440) | **вид есть в коде** (решение 440; включение — после записи узла) | Instadapp/fluid-contracts-public@404d73f deployments/arbitrum: DexFactory 0x91716C4EDA1Fb55e84Bf8b4c7085f84285c19085, DexReservesResolver 0x05Bd8269A20C472b148246De20E6852091BF16Ff, Liquidity 0x52Aa899454998Be5b000Ad077a46Bbe360F4e497 | T433-078 |
| 79 | `ekubo-v3-arbitrum` | Ekubo V3 (Arbitrum) | Ekubo | — | **семейства нет в коде** | не сверены | T433-079 |
| 80 | `ramses-legacy-arbitrum` | Ramses Legacy (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-080 |
| 81 | `ramses-v3-arbitrum` | Ramses V3 (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-081 |
| 82 | `rubicon-clmm-arbitrum` | Rubicon CLMM (Arbitrum) | — | — | **семейство не установлено** | не сверены | T433-082 |
| 83 | `aero-slipstream-arbitrum` | Aero Slipstream (Arbitrum) | Slipstream — ABI на Arbitrum не сверен (docs/notes/arbitrum/docs-check-2026-11-20.md) | `slipstream` — src/venues/bsc_dex/pools/families.rs:385 | **вид есть в коде** → **нужен код — развёртывание Aero «Slipstream V3» найдено (Dromos-Labs/metadex-slipstream-public cf1ae46f, cl-arbitrum.json), комиссия от крючка обмена — новое семейство (T448-09, решение 451)** | адреса, источники, статус — docs/notes/arbitrum/dex-settings-arbitrum.md (решение 448) | T433-083 |
| 84 | `camelot-v4` | Camelot V4 | Algebra Integral (docs/notes/arbitrum/docs-check-2026-11-20.md) | `algebra_integral` — src/venues/bsc_dex/pools/families.rs:331 | **вид есть в коде** → **готово** (решение 448) | адреса, источники, статус — docs/notes/arbitrum/dex-settings-arbitrum.md (решение 448) | T433-084 |

## 2. DefiLlama, категории «Dexs» и «DEX Aggregator» — все 107 записей и их строка в GeckoTerminal

Сопоставление — по названию (Ramses CL / CL V2 / Legacy V2, Archly V2, Chronos V2 с id GeckoTerminal не сопоставлены —
идут отдельными строками таблицы 3; сверка соответствия — внутри их задач).

| DefiLlama | slug | Категория | В GeckoTerminal |
| --- | --- | --- | --- |
| Curve DEX | `curve-dex` | Dexs | `curve_arbitrum` (строка таблицы 1) |
| SushiSwap | `sushiswap` | Dexs | `sushiswap_arbitrum` (строка таблицы 1) |
| KyberSwap Classic | `kyberswap-classic` | Dexs | `kyberswap_classic_arbitrum` (строка таблицы 1) |
| Shell Protocol | `shell-protocol` | Dexs | нет → таблица 3, T433-085 |
| DODO AMM | `dodo-amm` | Dexs | нет → таблица 3, T433-086 |
| PancakeSwap AMM | `pancakeswap-amm` | Dexs | `pancakeswap-v2-arbitrum` (строка таблицы 1) |
| Saddle Finance | `saddle-finance` | Dexs | нет → таблица 3, T433-087 |
| Integral | `integral` | Dexs | нет → таблица 3, T433-088 |
| Swapr V2 | `swapr-v2` | Dexs | `swapr_arbitrum` (строка таблицы 1) |
| WardenSwap | `wardenswap` | Dexs | нет → таблица 3, T433-089 |
| ApeSwap AMM | `apeswap-amm` | Dexs | `apeswap-arbitrum` (строка таблицы 1) |
| Elk | `elk` | Dexs | `elk_finance_arbitrum` (строка таблицы 1) |
| Joe DEX | `joe-dex` | Dexs | нет → таблица 3, T433-090 |
| Clipper | `clipper` | Dexs | нет → таблица 3, T433-091 |
| Rubicon | `rubicon` | Dexs | нет → таблица 3, T433-092 |
| ZipSwap | `zipswap` | Dexs | нет → таблица 3, T433-093 |
| Hashflow | `hashflow` | DEX Aggregator | нет → таблица 3, T433-094 |
| WOOFi Swap | `woofi-swap` | Dexs | нет → таблица 3, T433-095 |
| Wombat Exchange | `wombat-exchange` | Dexs | нет → таблица 3, T433-096 |
| Swapsicle V1 | `swapsicle-v1` | Dexs | нет → таблица 3, T433-097 |
| iZiSwap | `iziswap` | Dexs | нет → таблица 3, T433-098 |
| Arbswap AMM | `arbswap-amm` | Dexs | `arbswap_arbitrum_one` (строка таблицы 1) |
| 0x | `0x` | DEX Aggregator | нет → таблица 3, T433-099 |
| Frax Swap | `frax-swap` | Dexs | `fraxswap_arbitrum` (строка таблицы 1) |
| Sushi Trident | `sushi-trident` | Dexs | нет → таблица 3, T433-100 |
| Uniswap V2 | `uniswap-v2` | Dexs | `uniswap-v2-arbitrum` (строка таблицы 1) |
| Uniswap V3 | `uniswap-v3` | Dexs | `uniswap_v3_arbitrum` (строка таблицы 1) |
| 3xcalibur | `3xcalibur` | Dexs | `3xcalibur` (строка таблицы 1) |
| Camelot V2 | `camelot-v2` | Dexs | `camelot` (строка таблицы 1) |
| Joe V2 | `joe-v2` | Dexs | `traderjoe-v2-arbitrum` (строка таблицы 1) |
| Zyberswap AMM | `zyberswap-amm` | Dexs | `zyberswap` (строка таблицы 1) |
| SolidLizard Dex | `solidlizard-dex` | Dexs | `solidlizard` (строка таблицы 1) |
| PancakeSwap StableSwap | `pancakeswap-stableswap` | Dexs | `pancakeswap-stableswap-arbitrum` (строка таблицы 1) |
| ZyberSwap Stableswap | `zyberswap-stableswap` | Dexs | нет → таблица 3, T433-101 |
| Clober V1 | `clober-v1` | Dexs | нет → таблица 3, T433-102 |
| Dexalot DEX | `dexalot-dex` | Dexs | нет → таблица 3, T433-103 |
| Zyberswap V3 | `zyberswap-v3` | Dexs | нет → таблица 3, T433-104 |
| AlienFi | `alienfi` | Dexs | `alienfi` (строка таблицы 1) |
| Balancer V2 | `balancer-v2` | Dexs | `balancer_arbitrum` (строка таблицы 1) |
| KyberSwap Elastic | `kyberswap-elastic` | Dexs | `kyberswap_elastic_arbitrum` (строка таблицы 1) |
| Lighter V1 | `lighter-v1` | Dexs | нет → таблица 3, T433-105 |
| Ramses Legacy | `ramses-legacy` | Dexs | `ramses-legacy-arbitrum` (строка таблицы 1) |
| Arbitrum Exchange V2 | `arbitrum-exchange-v2` | Dexs | нет → таблица 3, T433-106 |
| SMARDEX AMM | `smardex-amm` | Dexs | `smardex-arbitrum` (строка таблицы 1) |
| Swapline V1 | `swapline-v1` | Dexs | нет → таблица 3, T433-107 |
| MM Finance Arbitrum | `mm-finance-arbitrum` | Dexs | `mm-finance-arbitrum` (строка таблицы 1) |
| Auragi Finance | `auragi-finance` | Dexs | `auragi` (строка таблицы 1) |
| PancakeSwap AMM V3 | `pancakeswap-amm-v3` | Dexs | `pancakeswap-v3-arbitrum` (строка таблицы 1) |
| SushiSwap V3 | `sushiswap-v3` | Dexs | `sushiswap-v3-arbitrum` (строка таблицы 1) |
| Camelot V3 | `camelot-v3` | Dexs | `camelot-v3` (строка таблицы 1) |
| Native Swap | `native-swap` | Dexs | нет → таблица 3, T433-108 |
| Joe V2.1 | `joe-v2.1` | Dexs | `traderjoe-v2-1-arbitrum` (строка таблицы 1) |
| Chronos V1 | `chronos-v1` | Dexs | `chronos` (строка таблицы 1) |
| E3 | `e3` | Dexs | нет → таблица 3, T433-109 |
| AirSwap | `airswap` | DEX Aggregator | нет → таблица 3, T433-110 |
| Arbitrum Exchange V3 | `arbitrum-exchange-v3` | Dexs | нет → таблица 3, T433-111 |
| Ramses CL | `ramses-cl` | Dexs | нет → таблица 3, T433-112 |
| Swaap Maker V2 | `swaap-maker-v2` | Dexs | нет → таблица 3, T433-113 |
| dexSWAP | `dexswap` | Dexs | `dexswap` (строка таблицы 1) |
| CrescentSwap | `crescentswap` | Dexs | `crescentswap` (строка таблицы 1) |
| SpartaDex | `spartadex` | Dexs | `spartadex` (строка таблицы 1) |
| Chronos V2 | `chronos-v2` | Dexs | нет → таблица 3, T433-114 |
| DackieSwap V3 | `dackieswap-v3` | Dexs | `dackieswap-v3-arbitrum` (строка таблицы 1) |
| Solidly V3 | `solidly-v3` | Dexs | `solidly-v3-arbitrum` (строка таблицы 1) |
| DackieSwap V2 | `dackieswap-v2` | Dexs | `dackieswap-v2-arbitrum` (строка таблицы 1) |
| Tegro | `tegro` | Dexs | нет → таблица 3, T433-115 |
| Thick | `thick` | Dexs | нет → таблица 3, T433-116 |
| Archly V2 | `archly-v2` | Dexs | нет → таблица 3, T433-117 |
| Horiza | `horiza` | Dexs | нет → таблица 3, T433-118 |
| DeltaSwap | `deltaswap` | Dexs | `deltaswap` (строка таблицы 1) |
| Mangrove | `mangrove` | Dexs | нет → таблица 3, T433-119 |
| Basin Exchange | `basin-exchange` | Dexs | нет → таблица 3, T433-120 |
| Maverick V2 | `maverick-v2` | Dexs | `maverick-v2-arbitrum` (строка таблицы 1) |
| Joe V2.2 | `joe-v2.2` | Dexs | `traderjoe-v2-2-arbitrum` (строка таблицы 1) |
| GMX V1 AMM | `gmx-v1-amm` | Dexs | нет → таблица 3, T433-121 |
| GMX AMM | `gmx-amm` | Dexs | нет → таблица 3, T433-122 |
| Level AMM | `level-amm` | Dexs | нет → таблица 3, T433-123 |
| Renegade | `renegade` | Dexs | нет → таблица 3, T433-124 |
| Vertex AMM | `vertex-amm` | Dexs | нет → таблица 3, T433-125 |
| KTX spot | `ktx-spot` | Dexs | нет → таблица 3, T433-126 |
| Raindex | `raindex` | Dexs | нет → таблица 3, T433-127 |
| Cables Finance | `cables-finance` | Dexs | нет → таблица 3, T433-128 |
| Nabla Finance | `nabla-finance` | Dexs | нет → таблица 3, T433-129 |
| Fluid DEX | `fluid-dex` | Dexs | `fluid-arbitrum` (строка таблицы 1) |
| Mach | `mach` | DEX Aggregator | нет → таблица 3, T433-130 |
| Balancer V3 | `balancer-v3` | Dexs | `balancer-v3-arbitrum` (строка таблицы 1) |
| Uniswap V4 | `uniswap-v4` | Dexs | `uniswap-v4-arbitrum` (строка таблицы 1) |
| Bunni V2 | `bunni-v2` | Dexs | нет → таблица 3, T433-131 |
| Skate AMM | `skate-amm` | Dexs | нет → таблица 3, T433-132 |
| Omni Exchange V3 | `omni-exchange-v3` | Dexs | `omni-exchange-v3-arbitrum` (строка таблицы 1) |
| Omni Exchange V2 | `omni-exchange-v2` | Dexs | `omni-exchange-v2-arbitrum` (строка таблицы 1) |
| Omni Exchange Flux | `omni-exchange-flux` | Dexs | нет → таблица 3, T433-133 |
| BrownFi V2 | `brownfi-v2` | Dexs | нет → таблица 3, T433-134 |
| Tristero Darkpool | `tristero-darkpool` | Dexs | нет → таблица 3, T433-135 |
| Alphix | `alphix` | Dexs | нет → таблица 3, T433-136 |
| Kaliber | `kaliber` | Dexs | нет → таблица 3, T433-137 |
| Metric V0 | `metric-v0` | Dexs | нет → таблица 3, T433-138 |
| BrownFi V3 | `brownfi-v3` | Dexs | нет → таблица 3, T433-139 |
| Ramses Legacy V2 | `ramses-legacy-v2` | Dexs | нет → таблица 3, T433-140 |
| Ramses CL V2 | `ramses-cl-v2` | Dexs | нет → таблица 3, T433-141 |
| 1inch Aqua | `1inch-aqua` | Dexs | нет → таблица 3, T433-142 |
| AutoRange | `autorange` | Dexs | нет → таблица 3, T433-143 |
| Metric V1 | `metric-v1` | Dexs | нет → таблица 3, T433-144 |
| Aero Slipstream | `aero-slipstream` | Dexs | `aero-slipstream-arbitrum` (строка таблицы 1) |
| PancakeSwap X | `pancakeswap-x` | Dexs | нет → таблица 3, T433-145 |
| Aero Basic | `aero-basic` | Dexs | нет → таблица 3, T433-146 |
| Camelot V4 | `camelot-v4` | Dexs | `camelot-v4` (строка таблицы 1) |

## 3. DEX из DefiLlama, которых нет в GeckoTerminal — 62

Пулы этих DEX индексатор справочника (GeckoTerminal, решение 58) не отдаёт: справочник пулов для них — часть задачи.

| № | DefiLlama | slug | Категория | Семейство (основание) | В коде (файл:строка) | Статус | Официальные адреса | Задача |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 85 | Shell Protocol | `shell-protocol` | Dexs | — | — | **семейство не установлено** | не сверены | T433-085 |
| 86 | DODO AMM | `dodo-amm` | Dexs | DODO PMM: DVM / DPP / DSP (docs/notes/arbitrum/docs-check-2026-11-20.md); пулы индексатор GeckoTerminal в сети arbitrum не отдаёт (в списке 84 DEX нет DODO) | `dodo_dvm` — src/venues/bsc_dex/pools/families.rs:321, `dodo_dpp` — src/venues/bsc_dex/pools/families.rs:322, `dodo_dsp` — src/venues/bsc_dex/pools/families.rs:323 | **вид есть в коде** → **готово (пулы — поиском по фабрике)** (решение 448) | адреса, источники, статус — docs/notes/arbitrum/dex-settings-arbitrum.md (решение 448) | T433-086 |
| 87 | Saddle Finance | `saddle-finance` | Dexs | Saddle StableSwap (протокол Saddle; ABI развёртывания Arbitrum не сверен) | `saddle_stableswap` — src/venues/bsc_dex/pools/families.rs:370 | **вид есть в коде** → **готово — математика = Nerve; swapStorage 7 полей и FlashLoan исправлены в виде (решение 451); запись узла — T448-07** | адреса, источники, статус — docs/notes/arbitrum/dex-settings-arbitrum.md (решение 448) | T433-087 |
| 88 | Integral | `integral` | Dexs | — | — | **семейство не установлено** | не сверены | T433-088 |
| 89 | WardenSwap | `wardenswap` | Dexs | — | — | **семейство не установлено** | не сверены | T433-089 |
| 90 | Joe DEX | `joe-dex` | Dexs | — | — | **семейство не установлено** | не сверены | T433-090 |
| 91 | Clipper | `clipper` | Dexs | — | — | **семейство не установлено** | не сверены | T433-091 |
| 92 | Rubicon | `rubicon` | Dexs | — | — | **семейство не установлено** | не сверены | T433-092 |
| 93 | ZipSwap | `zipswap` | Dexs | — | — | **семейство не установлено** | не сверены | T433-093 |
| 94 | Hashflow | `hashflow` | DEX Aggregator | категория DefiLlama «DEX Aggregator» — свой пул не установлен | — | **не пул** | не сверены | T433-094 |
| 95 | WOOFi Swap | `woofi-swap` | Dexs | WOOFi WooPPV2 (docs/notes/arbitrum/docs-check-2026-11-20.md) | `woofi_v2` — src/venues/bsc_dex/pools/families.rs:379 | **вид есть в коде** → **готово — оракул = WooracleV2_2 (Sourcify), реализация 0x7b5f…4281 (Sourcify proxyResolution); слот и селекторы своим узлом — T448-08 (решение 451)** | адреса, источники, статус — docs/notes/arbitrum/dex-settings-arbitrum.md (решение 448) | T433-095 |
| 96 | Wombat Exchange | `wombat-exchange` | Dexs | — | — | **семейство не установлено** | не сверены | T433-096 |
| 97 | Swapsicle V1 | `swapsicle-v1` | Dexs | — | — | **семейство не установлено** | не сверены | T433-097 |
| 98 | iZiSwap | `iziswap` | Dexs | iZiSwap (ABI развёртывания Arbitrum не сверен) | `iziswap` — src/venues/bsc_dex/pools/families.rs:349 | **вид есть в коде** → **нужен код — источник пулов (T448-05)** (решение 448) → **код есть, сверка — T450-01 (запись)** (решение 450: поиск по фабрике pool(x, y, fee)) | адреса, источники, статус — docs/notes/arbitrum/dex-settings-arbitrum.md (решение 448) | T433-098 |
| 99 | 0x | `0x` | DEX Aggregator | категория DefiLlama «DEX Aggregator» — свой пул не установлен | — | **не пул** | не сверены | T433-099 |
| 100 | Sushi Trident | `sushi-trident` | Dexs | — | — | **семейство не установлено** | не сверены | T433-100 |
| 101 | ZyberSwap Stableswap | `zyberswap-stableswap` | Dexs | — | — | **семейство не установлено** | не сверены | T433-101 |
| 102 | Clober V1 | `clober-v1` | Dexs | — | — | **семейство не установлено** | не сверены | T433-102 |
| 103 | Dexalot DEX | `dexalot-dex` | Dexs | книга ордеров — не пул (docs/notes/arbitrum/docs-check-2026-11-20.md) | — | **не пул** | не сверены | T433-103 |
| 104 | Zyberswap V3 | `zyberswap-v3` | Dexs | — | — | **семейство не установлено** | не сверены | T433-104 |
| 105 | Lighter V1 | `lighter-v1` | Dexs | — | — | **семейство не установлено** | не сверены | T433-105 |
| 106 | Arbitrum Exchange V2 | `arbitrum-exchange-v2` | Dexs | — | — | **семейство не установлено** | не сверены | T433-106 |
| 107 | Swapline V1 | `swapline-v1` | Dexs | — | — | **семейство не установлено** | не сверены | T433-107 |
| 108 | Native Swap | `native-swap` | Dexs | RFQ — не пул (docs/notes/arbitrum/docs-check-2026-11-20.md) | — | **не пул** | не сверены | T433-108 |
| 109 | E3 | `e3` | Dexs | — | — | **семейство не установлено** | не сверены | T433-109 |
| 110 | AirSwap | `airswap` | DEX Aggregator | категория DefiLlama «DEX Aggregator» — свой пул не установлен | — | **не пул** | не сверены | T433-110 |
| 111 | Arbitrum Exchange V3 | `arbitrum-exchange-v3` | Dexs | — | — | **семейство не установлено** | не сверены | T433-111 |
| 112 | Ramses CL | `ramses-cl` | Dexs | — | — | **семейство не установлено** | не сверены | T433-112 |
| 113 | Swaap Maker V2 | `swaap-maker-v2` | Dexs | — | — | **семейство не установлено** | не сверены | T433-113 |
| 114 | Chronos V2 | `chronos-v2` | Dexs | — | — | **семейство не установлено** | не сверены | T433-114 |
| 115 | Tegro | `tegro` | Dexs | — | — | **семейство не установлено** | не сверены | T433-115 |
| 116 | Thick | `thick` | Dexs | — | — | **семейство не установлено** | не сверены | T433-116 |
| 117 | Archly V2 | `archly-v2` | Dexs | — | — | **семейство не установлено** | не сверены | T433-117 |
| 118 | Horiza | `horiza` | Dexs | — | — | **семейство не установлено** | не сверены | T433-118 |
| 119 | Mangrove | `mangrove` | Dexs | — | — | **семейство не установлено** | не сверены | T433-119 |
| 120 | Basin Exchange | `basin-exchange` | Dexs | — | — | **семейство не установлено** | не сверены | T433-120 |
| 121 | GMX V1 AMM | `gmx-v1-amm` | Dexs | GMX — не пул (docs/notes/arbitrum/docs-check-2026-11-20.md) | — | **не пул** | не сверены | T433-121 |
| 122 | GMX AMM | `gmx-amm` | Dexs | GMX — не пул (docs/notes/arbitrum/docs-check-2026-11-20.md) | — | **не пул** | не сверены | T433-122 |
| 123 | Level AMM | `level-amm` | Dexs | — | — | **семейство не установлено** | не сверены | T433-123 |
| 124 | Renegade | `renegade` | Dexs | — | — | **семейство не установлено** | не сверены | T433-124 |
| 125 | Vertex AMM | `vertex-amm` | Dexs | — | — | **семейство не установлено** | не сверены | T433-125 |
| 126 | KTX spot | `ktx-spot` | Dexs | — | — | **семейство не установлено** | не сверены | T433-126 |
| 127 | Raindex | `raindex` | Dexs | — | — | **семейство не установлено** | не сверены | T433-127 |
| 128 | Cables Finance | `cables-finance` | Dexs | — | — | **семейство не установлено** | не сверены | T433-128 |
| 129 | Nabla Finance | `nabla-finance` | Dexs | — | — | **семейство не установлено** | не сверены | T433-129 |
| 130 | Mach | `mach` | DEX Aggregator | категория DefiLlama «DEX Aggregator» — свой пул не установлен | — | **не пул** | не сверены | T433-130 |
| 131 | Bunni V2 | `bunni-v2` | Dexs | — | — | **семейство не установлено** | не сверены | T433-131 |
| 132 | Skate AMM | `skate-amm` | Dexs | — | — | **семейство не установлено** | не сверены | T433-132 |
| 133 | Omni Exchange Flux | `omni-exchange-flux` | Dexs | — | — | **семейство не установлено** | не сверены | T433-133 |
| 134 | BrownFi V2 | `brownfi-v2` | Dexs | — | — | **семейство не установлено** | не сверены | T433-134 |
| 135 | Tristero Darkpool | `tristero-darkpool` | Dexs | — | — | **семейство не установлено** | не сверены | T433-135 |
| 136 | Alphix | `alphix` | Dexs | — | — | **семейство не установлено** | не сверены | T433-136 |
| 137 | Kaliber | `kaliber` | Dexs | — | — | **семейство не установлено** | не сверены | T433-137 |
| 138 | Metric V0 | `metric-v0` | Dexs | Metric (на BSC — Metric OMM, `metric_omm` — src/venues/bsc_dex/pools/families.rs:347); версия V0 не сверена | — | **семейство не установлено** | не сверены | T433-138 |
| 139 | BrownFi V3 | `brownfi-v3` | Dexs | — | — | **семейство не установлено** | не сверены | T433-139 |
| 140 | Ramses Legacy V2 | `ramses-legacy-v2` | Dexs | — | — | **семейство не установлено** | не сверены | T433-140 |
| 141 | Ramses CL V2 | `ramses-cl-v2` | Dexs | — | — | **семейство не установлено** | не сверены | T433-141 |
| 142 | 1inch Aqua | `1inch-aqua` | Dexs | — | — | **семейство не установлено** | не сверены | T433-142 |
| 143 | AutoRange | `autorange` | Dexs | — | — | **семейство не установлено** | не сверены | T433-143 |
| 144 | Metric V1 | `metric-v1` | Dexs | Metric (на BSC — Metric OMM, `metric_omm` — src/venues/bsc_dex/pools/families.rs:347); версия на Arbitrum не сверена (docs/notes/arbitrum/docs-check-2026-11-20.md) | — | **семейство не установлено** | не сверены | T433-144 |
| 145 | PancakeSwap X | `pancakeswap-x` | Dexs | — | — | **семейство не установлено** | не сверены | T433-145 |
| 146 | Aero Basic | `aero-basic` | Dexs | — | — | **семейство не установлено** | не сверены | T433-146 |

## 4. Семейства, которых нет в коде (задачи кода)

- T433-F1 Fluid DEX (T1) — СДЕЛАНО в коде (решение 440, вид `fluid_t1`; осталось: запись узла tools/arbitrum_record/fluid_t1.py, сборка исполнителя v4 — docs/TASKS.md «Решение 440») — новое семейство пулов: математика, чтение, события, проверка, квотер, шаг исполнителя; файлы по образцу существующих видов (pools/families.rs, pools/load_*, events_*, verify_*, math/), тесты — на записанных ответах своего узла Arbitrum.
- T433-F2 Balancer V3 — новое семейство (хранилище V3, крючки): математика, чтение, события, проверка, квотер, шаг исполнителя; файлы по образцу существующих видов (pools/families.rs, pools/load_*, events_*, verify_*, math/), тесты — на записанных ответах своего узла Arbitrum. **Решения 441, 452: сделано в коде** (вид `balancer_v3`: все виды пулов официальных фабрик Arbitrum и крючки StableSurge / MEV Capture, PoolRegistered хранилища; не сделано — обмен через буфер ERC4626: сумма — previewDeposit / previewRedeem самой обёртки, своя у каждой, нужен новый шаг исполнителя — вопрос в docs/TASKS.md «Решение 452», п. 2; запись узла — tools/arbitrum_record/balancer_v3.py, тест tests/arbitrum_balancer_v3.rs — #[ignore] до записи).
- T433-F3 Curve Twocrypto / Tricrypto (crypto-пулы) — новое семейство; файлы по образцу существующих видов (pools/families.rs, pools/load_*, events_*, verify_*, math/), тесты — на записанных ответах своего узла Arbitrum.
- T433-F4 Camelot V2 — пара с направленными комиссиями и стабильными парами — новое семейство (или расширение V2); файлы по образцу существующих видов (pools/families.rs, pools/load_*, events_*, verify_*, math/), тесты — на записанных ответах своего узла Arbitrum.
- T433-F5 Camelot V3 — Algebra 1.9 с направленными комиссиями (feeZtO/feeOtZ) — расширение вида Algebra; файлы по образцу существующих видов (pools/families.rs, pools/load_*, events_*, verify_*, math/), тесты — на записанных ответах своего узла Arbitrum.
- T433-F6 Liquidity Book v2.0 (LFJ V2) — новое семейство (в коде только v2.1); файлы по образцу существующих видов (pools/families.rs, pools/load_*, events_*, verify_*, math/), тесты — на записанных ответах своего узла Arbitrum.
- T433-F7 Liquidity Book v2.2 (LFJ V2.2) — новое семейство или расширение lb_v21; файлы по образцу существующих видов (pools/families.rs, pools/load_*, events_*, verify_*, math/), тесты — на записанных ответах своего узла Arbitrum.
- T433-F8 Ekubo — новое семейство; файлы по образцу существующих видов (pools/families.rs, pools/load_*, events_*, verify_*, math/), тесты — на записанных ответах своего узла Arbitrum.

## 5. Записи DefiLlama других категорий (не DEX спота) — 53

Volmex (Yield), Pendle V2 (Yield), NFTX (NFT Marketplace), Overtime (Prediction Market), Superfluid (Payments), LlamaPay (Payments), El Dorado Exchange (Derivatives), SpaceDex (Derivatives), Sudoswap V2 (NFT Marketplace), Sablier Lockup (Payments), Sweep n Flip (NFT Marketplace), YFX (Derivatives), BLEX (Derivatives), Primex Finance (Derivatives), Ryze.Fi (Derivatives), Garden (Cross Chain Bridge), Lexer Markets (Derivatives), Dinari (RWA), Secured Finance Lending (Lending), Curve LlamaLend (Lending), Fjord V2 (Launchpad), Fjord V1 (Launchpad), Equation V3 (Derivatives), Whales Market (OTC Marketplace), C3 Exchange (Cross Chain Bridge), YFX V4 (Derivatives), Spectra V2 (Yield), Napier (Yield), Ducata (Yield), SpaceWhale (Derivatives), GMGN (Trading App), Sablier Flow (Payments), Swaps.io (Bridge), Permute (Bridge), Rain (Prediction Market), Holyheld Card (Crypto Card Issuer), Bitget Wallet Card (Crypto Card Issuer), Printr (Launchpad), RedotPay Card (Crypto Card Issuer), Cypher Card (Crypto Card Issuer), Bleap Card (Crypto Card Issuer), Nexo Card (Crypto Card Issuer), Spritz Card (Crypto Card Issuer), x402 (Payments), Uniswap Auctions (Launchpad), SafePal Card (Crypto Card Issuer), DashsWallet (Portfolio Tracker), Kolo Card (Crypto Card Issuer), AxiumPass (Payments), StonkBrokers Safe Launch (Launchpad), Adexto (Launchpad), Vailox (OTC Marketplace), SoneVibe (Lending).
Деривативы (Derivatives) — не спот: фьючерсов в проекте нет (CLAUDE.md).
