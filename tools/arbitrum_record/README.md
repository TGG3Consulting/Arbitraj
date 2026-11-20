# tools/arbitrum_record — запись ответов своего узла Arbitrum One для тестов

## Одна команда (решение 459)

Из корня репозитория, адрес — HTTP JSON-RPC своего узла Arbitrum (WebSocket не нужен):

- Windows: `py tools\arbitrum_record\record_all.py --rpc http://АДРЕС_УЗЛА:8547`
- Linux: `python3 tools/arbitrum_record/record_all.py --rpc http://АДРЕС_УЗЛА:8547`

Нужен только Python 3.8+ (стандартная библиотека; `pip` не нужен — HTTP через `stdhttp.py`). Вопросов в процессе нет.
Скрипты по очереди пишут свои папки `tests/data/…`; упавший скрипт не останавливает остальные. В конце — сводка (что
записано, что упало и почему) и строка «Архив для отправки: …zip» — этот zip прислать. Журналы каждого скрипта — в
`arbitrum_record_<время>/logs/` (в git не идут). Ключей и приватных данных скрипты не просят: только чтение узла
(eth_call, eth_getLogs, eth_getCode, eth_getStorageAt, eth_simulateV1), транзакций нет.

Необязательно: `--wallet 0x…` — адрес кошелька для записи приёмки v4 (адрес, не ключ; без него — пустой адрес);
`--ekubo-from-block N` — блок развёртывания ядра Ekubo (без него ищется по eth_getCode узла; узел без истории
состояния ответит ошибкой — тогда ekubo_more.py пропускается с причиной в сводке); `--only ИМЯ` / `--skip ИМЯ` —
выбрать скрипты; `--timeout С` — предел одного скрипта. Нужен доступ в интернет к api.geckoterminal.com (списки
пулов kyber_elastic_v1, fluid_t1, balancer_v3, algebra_dir_fee, ekubo). Приёмке v4 нужен eth_simulateV1 на узле —
без него exec_v4_live.py выходит с причиной.

Узел недоступен — сводка с причиной и zip всё равно пишутся (код выхода 1). Дата папок записи одна на весь запуск
(ekubo.py и ekubo_more.py — в одну папку). Переводы строк contracts/out/ и tests/data/ git не меняет (.gitattributes,
`-text`): SHA-256 кода исполнителя на Windows совпадает с ArbExecutor.sha256.

Что запускается (адреса — `record_steps.py`, те же, что в шапках скриптов и ниже): sub_dex, kyber_elastic_v1, saddle,
woofi, fluid_t1, balancer_v3, curve_crypto, curve_plain, camelot_v2, algebra_dir_fee, lb_v20, lb_v22, ekubo, ekubo_more,
exec_v4_live.

Тесты проекта — только на реально записанных ответах (CLAUDE.md). У разработчика нет доступа к узлу Arbitrum: после
записи папки `tests/data/arbitrum_*_<дата>/` и `tests/data/executor_v4_live_<дата>/` коммитятся, с тестов снимается
`#[ignore]` (список — docs/TASKS.md, T456-03).

Каждый скрипт пишет JSON-файлы и README.md (блок, хеши SHA-256) в `tests/data/arbitrum_<семейство>_<дата>/`; адрес
узла — аргумент (`--rpc` или иной — см. шапку скрипта); адреса контрактов — из официальных источников (в шапке скрипта).

| Скрипт | Решение | Что пишет | Тест |
| --- | --- | --- | --- |
| `kyber_elastic_v1.py` | 451 (T448-06) | пулы KyberSwap Elastic v1: состояние, тики, QuoterV2, getPool фабрики, логи | `tests/arbitrum_kyber_elastic_v1_live.rs` |
| `saddle.py` | 451 (T448-07) | пулы Saddle: монеты, балансы, A, swapStorage (7 слов), calculateSwap, логи | `tests/arbitrum_saddle_live.rs` |
| `woofi.py` | 451 (T448-08) | WooPPV2.2: слот EIP-1967, селекторы в коде реализации, state() оракула, query | `tests/arbitrum_woofi_live.rs` |
| `sub_dex.py` | 450 (T448-01…05) | KyberSwap Classic DMM / Static Fee / Legacy (события создания, getTradeInfo, getPools, getAmountsOut роутеров), Balancer V2 по фабрикам с версией библиотек (состояние, isPoolFromFactory, queryBatchSwap), iZiSwap (fee2pointDelta, NewPool, реестр pool(x, y, fee)), код Universal Router PancakeSwap v3 — tests/data/arbitrum_sub_dex_<дата>/ | `tests/arbitrum_sub_dex_live.rs` |
| `fluid_t1.py` | 440 | Fluid DEX T1 (решение 440): состояние всех пулов резолвера на одном блоке и лесенка обменов (swapIn пула → 0x…dEaD, estimateSwapIn резолвера) — для tests/arbitrum_fluid_t1.rs | `tests/arbitrum_fluid_t1.rs` |
| `balancer_v3.py` | 441 | Решения 441, 452 (T433-F2): пулы Balancer V3 всех видов — все вызовы бота к пулу (хранилище, фабрики из списков, виды пулов, крючок, прекомпайлы) и ответы Router V3 querySwapSingleTokenExactIn / ExactOut на одном блоке; тест — tests/arbitrum_balancer_v3.rs | `tests/arbitrum_balancer_v3.rs` |
| `curve_crypto.py` | 442 | Curve crypto-пулы (решения 442, 453): Twocrypto-NG (v2.1.0 и v2.0.0 — по MATH()), Tricrypto-NG, фабрика crypto v1, tricrypto 2021 — ответы на вызовы бота по calldata, проверка (get_coins, version, MATH, get_decimals), eth_getCode реализаций / математики / пулов, balanceOf и ETH пулов, decimals монет v1, лесенка get_dy — tests/data/arbitrum_curve_crypto_<дата>/ | tests/arb_curve_crypto_live.rs (NG), tests/arb_curve_crypto_v1_live.rs (v1) | `tests/arb_curve_crypto_live.rs`, `tests/arb_curve_crypto_v1_live.rs` |
| `curve_plain.py` | 472 (T450-02) | Curve StableSwap 2021 (вид curve_stableswap_plain): 2pool и plain-пулы фабрики Curve Sidechain/L2 на Plain2/3/4Basic — ответы на вызовы бота по calldata одним пакетом на блоке B (coins / balances / A_precise / fee и геттеры рампы), decimals, get_dy на долях баланса, код 2pool и реализаций (селектор exchange из 4 аргументов), логи узким окном — tests/data/arbitrum_curve_plain_<дата>/ | `tests/arbitrum_curve_plain_live.rs` |
| `curve_plain_pools.py`, `curve_sim.py` | 472 (доделка) | Помощники curve_plain.py: адреса реализаций (plain и мета), пулы фабрики, вызовы бота по (адрес, calldata); симуляция exchange — eth_call с подменой баланса и разрешения у пустого адреса одним пакетом (и для curve_crypto.py — exchange_sim) | `tests/arbitrum_curve_plain_live.rs`, `tests/arb_curve_crypto_tweak_live.rs` |
| `camelot_v2.py` | 443 | Решение 443 (T433-F4): Camelot V2 — пары фабрики (резервы, комиссии входа, stableSwap, множители точности, getPair, код), getAmountOut пары на суммах в обе стороны на одном блоке; логи Sync, FeePercentUpdated, SetStableSwap, PairCreated. Запуск: `python3 tools/arbitrum_record/camelot_v2.py http://УЗЕЛ:8547 --factory 0x6EcCab422D763aC031210895C81787E87B43A652` (из корня репозитория) | — |
| `lb_v20.py` | 445 | Liquidity Book v2.0 (LFJ V2, вид lb_v20, решение 445) | фабрика v2.0, состояние пулов, все бины, getSwapOut роутера v2.0, слоты условия, логи; тест — tests/bsc_dex_lb_v20_arbitrum.rs | `tests/bsc_dex_lb_v20_arbitrum.rs` |
| `lb_v22.py` | 446 | Liquidity Book V2.2 — LFJ V2.2 (решение 446, T433-F7): `python3 tools/arbitrum_record/lb_v22.py http://УЗЕЛ:8547 --factory 0xb43120c4745967fa9b93E79C149E66B0f2D6Fe0c` | tests/arb_dex_lb_v22_live.rs | `tests/arb_dex_lb_v22_live.rs` |
| `ekubo.py` | 447 | Ekubo (решение 447, T433-F8): пулы GeckoTerminal ekubo-v3-arbitrum и/или `--pool`, ключи PoolKeyIndex, Core.sload (состояние, битмап, тики), MEVCapture.sload, MEVCaptureRouter.quote, логи ядра | tests/arbitrum_ekubo_live.rs | `tests/arbitrum_ekubo_live.rs` |
| `curve_crypto_rpc.py`, `curve_crypto_pools.py` | 453 | модули `curve_crypto.py` (ревью 453, файлы ≤ ~200 строк): keccak256, ABI и JSON-RPC узла; чтение пулов (вызовы бота, лесенка get_dy, код, holdings, пулы фабрик) — запускать не нужно, их импортирует `curve_crypto.py` | — |
| `ekubo_more.py` | 454 | Ekubo, дополнительная запись (после `ekubo.py`, та же папка tests/data/arbitrum_ekubo_<дата>/): PoolInitialized ядра, пулы с нативным ETH, пулы TWAMM, замер нагрузки подписки ядра | `tests/arbitrum_ekubo_more_live.rs` |
| `record_all.py`, `record_steps.py` | 459 | одна команда: все скрипты ниже по очереди, сводка, zip | — |
| `exec_v4_live.py` (+ `v4live_rpc.py`, `v4live_slots.py`, `v4live_mgr.py`, `v4live_old.py`, `v4live_cases.py`, `v4live_write.py`) | 400, 459 (T457-02) | приёмка исполнителя v4 на живых пулах: eth_simulateV1 с подменой кода (contracts/out/ArbExecutor.bin-runtime) на пустом адресе, (а) покупка / (б) продажа по видам 19 Uniswap V4 и 23 Balancer V3 (их судит правило 400; у 23 — и пулы с крючком, которые бот берёт: static / StableSurge / MEV Capture, по 2 на вид, `--bv3-per-hook`; вида нет — «нет пулов») (у Balancer V3 квотер после (а) и продажа — две симуляции: Vault.quote состояние не откатывает) и 22 Fluid, 24 LB v2.0, 1 Uniswap V3, 0 Uniswap V2 — tests/data/executor_v4_live_<дата>/ (README с SHA-256 кода и chainId — запись судит кошельки только своей сети, summary.json, hooks_reject.json, calls.json, slots.json, pools.json, raw_rpc.json) | правило — src/venues/bsc_dex/exec/executor/live_rule.rs (см. docs/TASKS.md T457-02) |
| `stdhttp.py` | 459 | HTTP стандартной библиотеки вместо пакета requests (все скрипты: `import stdhttp as requests`) | — |
| `algebra_dir_fee.py` | 444 | Algebra V1.9 с направленными комиссиями (T433-F5; Camelot V3) — команда и хеши кода пула — раздел ниже | `tests/arb_dex_algebra_dir.rs` |

## algebra_dir_fee.py — Algebra V1.9 с направленными комиссиями (решение 444, T433-F5; Camelot V3)

Адреса — docs.camelot.exchange/contracts/arbitrum/one-mainnet («AMM V3»): AlgebraFactory
`0x1a3c9B1d2F0529D97f2afC5136Cc23e58f1FD35B`, Quoter `0x0Fc73040b26E9bC8514fA028D998E73A254Fa76E`.

Хеши кода пула (`--init-hash`) — все значения POOL_INIT_CODE_HASH в истории ветки direction-based-fee
cryptoalgebra/AlgebraV1.9 до a45417e (src/core/contracts/AlgebraFactory.sol; коммит, где значение впервые;
«направленные» — feeZto/feeOtz в base/PoolState.sol, «битмап» — TickTable.sol со сжатием на TICK_SPACING или без):

| Хеш | Коммит | Направленные комиссии | Битмап тиков |
| --- | --- | --- | --- |
| 0x77066034e31fe5172b86f4cf68f7e01dd6954f2e8292e6796c9bf7e99dbae477 | 0d5612b (2023-01-10) | да | сжатый (60) |
| 0xb785aaed76f65b185ee1fe081ef2233b7473b6c47bc008865f64dac83f4982f4 | a3dc950 (2023-01-10; начало аудита Paladin 14.06.2023) | да | сжатый (60) |
| 0xb994c319655d0a2474f63ef4c8c5c9ee051c08ddd25e75c6a36c89e8a2a229e7 | affd004 (2023-06-05, ветка v1-modified) | нет | сырой |
| 0x4e92c1fd81a5c8ce82f5a90742e3376c5b258017e149fd0629068ecc9aab43c6 | 82ef500 (2023-06-05, ветка v1-modified) | нет | сырой |
| 0xac97ff1b66ef7517262cbf16db0195b400714e8ecb499fc86f349e77f50ae89b | 4ada8b0 (2023-06-06) | да | сырой |
| 0xdcb95b809dca000f4ce04aa004b723fef2dbb3704db3c0389699bc70f0e42c79 | ade9e23 (2023-06-06, ветка v1-modified) | нет | сырой |
| 0x0ca66607e4d0ece7798fce9ae13cad1a28be1eee443451573f92ba6e3bcd92b9 | 237524b (2023-06-06; конец аудита Paladin 14.06.2023) | да | сырой |
| 0x5d100e2dbf4b80b9451c29380def0d6c4abeb3db0d166c81fda155a89e106dbd | 4c885a5 (2023-06-13) | да | сырой |
| 0xab9acbcae566bd662fb801895320dc3dd170e64394f78600b969ab128a3a40fd | cd4fd3b (2023-06-14) | да | сырой |
| 0x6c1bebd370ba84753516bc1393c0d0a6c645856da55f5393ac8ab3d6dbc861d3 | c63c18f (2023-06-14) … a45417e | да | сырой |

Аудит «Algebra Protocol Tick Spacing Upgrade» (Paladin, 14.06.2023; docs.camelot.exchange/references/audits, раздел
AMM V3) — сравнение a3dc950f…237524b репозитория cryptoalgebra/Algebra; оба коммита есть в ветке direction-based-fee
AlgebraV1.9. Какой код у пулов Camelot V3 — отвечает только запись (совпадение CREATE2).

Код бота читает СЫРОЙ битмап (load_algebra.rs). Тест `tests/arb_dex_algebra_dir.rs` проверяет, что адрес каждого
записанного пула = CREATE2(poolDeployer, keccak256(abi.encode(token0, token1)), хеш) для хеша версии с направленными комиссиями и сырым битмапом.

Команда (из корня репозитория; адрес узла — свой):

```
python3 tools/arbitrum_record/algebra_dir_fee.py --rpc http://<узел>:8547 \
  --factory 0x1a3c9B1d2F0529D97f2afC5136Cc23e58f1FD35B \
  --quoter 0x0Fc73040b26E9bC8514fA028D998E73A254Fa76E \
  --gecko-dex camelot-v3 --gecko-pools 5 \
  --init-hash 0x77066034e31fe5172b86f4cf68f7e01dd6954f2e8292e6796c9bf7e99dbae477 \
  --init-hash 0xb785aaed76f65b185ee1fe081ef2233b7473b6c47bc008865f64dac83f4982f4 \
  --init-hash 0xb994c319655d0a2474f63ef4c8c5c9ee051c08ddd25e75c6a36c89e8a2a229e7 \
  --init-hash 0x4e92c1fd81a5c8ce82f5a90742e3376c5b258017e149fd0629068ecc9aab43c6 \
  --init-hash 0xac97ff1b66ef7517262cbf16db0195b400714e8ecb499fc86f349e77f50ae89b \
  --init-hash 0xdcb95b809dca000f4ce04aa004b723fef2dbb3704db3c0389699bc70f0e42c79 \
  --init-hash 0x0ca66607e4d0ece7798fce9ae13cad1a28be1eee443451573f92ba6e3bcd92b9 \
  --init-hash 0x5d100e2dbf4b80b9451c29380def0d6c4abeb3db0d166c81fda155a89e106dbd \
  --init-hash 0xab9acbcae566bd662fb801895320dc3dd170e64394f78600b969ab128a3a40fd \
  --init-hash 0x6c1bebd370ba84753516bc1393c0d0a6c645856da55f5393ac8ab3d6dbc861d3 \
  --words 8 --bps 1 --bps 10 --bps 100 --log-blocks 20000 \
  --vq-runtime contracts/out/VirtualQuoter.bin-runtime --out tests/data
```

`--words 8` (слов сырого битмапа в каждую сторону, ≈ ±20% цены), `--bps` (доли баланса входа пула: 0,01%, 0,1%, 1%) и
`--log-blocks 20000` (≈ 1,4 ч при блоке 250 мс) — значения для теста, не настройки бота; при отказе узла по диапазону
eth_getLogs — уменьшить. Другие DEX Arbitrum на Algebra (кандидаты — docs/TASKS.md «Решение 444», вопрос 3): тот же
скрипт со своей фабрикой — совпадение CREATE2 с одним из хешей = пулы этой версии кода.
