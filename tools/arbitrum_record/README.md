# tools/arbitrum_record — запись ответов своего узла Arbitrum One для тестов

## Одна команда (решение 459)

Из корня репозитория, адрес — HTTP JSON-RPC своего узла Arbitrum (WebSocket не нужен):

- Windows: `py tools\arbitrum_record\record_all.py --rpc http://АДРЕС_УЗЛА:8547`
- Linux: `python3 tools/arbitrum_record/record_all.py --rpc http://АДРЕС_УЗЛА:8547`

Нужен только Python 3.8+ (стандартная библиотека; `pip` не нужен — HTTP через `stdhttp.py`). Вопросов в процессе нет.
Скрипты по очереди пишут свои папки `tests/data/…`; упавший скрипт не останавливает остальные. В конце — сводка (что
записано, что упало и почему) и строка «Архив для отправки: …zip» — этот zip прислать. Журналы каждого скрипта — в
`arbitrum_record_<время>/logs/` (в git не идут). Ключей и приватных данных скрипты не просят: только чтение узла
(eth_call, eth_getLogs, eth_getCode, eth_getStorageAt, eth_getBlockByNumber, eth_simulateV1), транзакций нет.

Необязательно: `--wallet 0x…` — адрес кошелька для записи приёмки v4 (адрес, не ключ; без него — пустой адрес);
`--ekubo-from-block N` — только переопределение блока ядра Ekubo (по умолчанию ekubo_more.py берёт его из официальных
broadcast-файлов Ekubo: `contracts/evmtest/vendor/ekubo/broadcast/DeployAll.s.sol/42161`, квитанция Core — блок
416469411); `--only ИМЯ,ИМЯ` / `--skip ИМЯ,ИМЯ` — выбрать скрипты (через запятую или несколько раз; неизвестное имя —
ошибка с перечнем); `--date ГГГГ-ММ-ДД` — дата папок записи (по умолчанию — сегодня UTC); `--timeout С` — предел одного
скрипта. Нужен доступ в интернет к api.geckoterminal.com (списки пулов kyber_elastic_v1, fluid_t1, balancer_v3,
algebra_dir_fee, ekubo, sub_dex). Приёмке v4 нужен eth_simulateV1 на узле — без него exec_v4_live.py выходит с причиной.

**Повтор после прогона 20.11.2026 (решение 460), одна строка:** `python3 tools/arbitrum_record/record_all.py --rpc
http://127.0.0.1:8547 --date 2026-11-20 --only sub_dex,balancer_v3,camelot_v2,lb_v22,ekubo,ekubo_more,exec_v4_live` (решение 479: ekubo_more.py идёт перед ekubo.py — ekubo.py берёт ключи пулов из pool_initialized.json)
(Windows — `py tools\arbitrum_record\record_all.py …`). `--date 2026-11-20` — ekubo.py и ekubo_more.py пишут в ту же
папку `tests/data/arbitrum_ekubo_2026-11-20/` (файлы у них разные: ekubo.py — README.md, meta.json, gt_pools.json,
pools.json, core_logs.json; ekubo_more.py — README_454.md, more_meta.json, pool_initialized.json, native_pools.json,
twamm_pools.json, core_load.json); ekubo.py повторяется сам (20.11 записал 0 пулов: GeckoTerminal отдаёт адрес пула
Ekubo как «<ядро>-<PoolId>», скрипт ждал только PoolId — теперь разбирается), его файлы перезаписываются новой записью;
sub_dex.py — 20.11 записал 0 пулов (за 2 000 000 блоков событий создания не было; поиск назад окнами тоже не годится —
раздел ниже); exec_v4_live.py — 20.11 вид 23 «нет пулов» (записи balancer_v3.py не было). camelot_v2.py ищет
FeePercentUpdated / SetStableSwap до блока первой пары фабрики — это долго (20.11: 18 окон по 2 000 000 блоков — 491 с),
теперь с потолком времени (раздел ниже).

**sub_dex без скана истории, потолок времени (решение 460, п. 9–10).** Факт узла Тиграна (Nitro, state-scheme=path):
sub_dex.py с поиском событий создания назад окнами eth_getLogs за 37 мин прошёл ~144 млн блоков из ~526 млн — на каждом
окне таймаут и деление. Теперь пулы — без скана истории (`sub_dex_find.py`): KyberSwap Classic — с конца списка
allPools(i) / allPoolsLength() трёх фабрик (eth_call, свежий блок; dmm-smart-contracts@e557b57 DMMFactory.sol L20,
L90-L92, KSFactory.sol L24, L127-L129; Legacy — тот же репозиторий до 195e3e9) и пулы GeckoTerminal
kyberswap_classic_arbitrum со своей factory(); Balancer V2 — перечисления у фабрик нет: пулы GeckoTerminal
balancer_arbitrum, фабрика — isPoolFromFactory = 1; iZiSwap — перечисления нет и в GeckoTerminal DEX нет: реестр
pool(x, y, fee) по парам токенов (токены пулов сети GeckoTerminal и `--izi-token`), factory() пула = фабрика. Событие
создания читает только тест KyberSwap Classic (`kyber_classic_created_and_math`: p["log"] каждого пула; тесты Balancer V2
и iZiSwap событий не читают) — берётся узким окном eth_getLogs (±1 ч, затем ±1 сут — `--created-margin`) вокруг блока
pool_created_at GeckoTerminal; блок по времени — бинпоиск по времени заголовков eth_getBlockByNumber. Заголовки — не
состояние: «The Nitro node databases have the raw data of all blocks, including pre-Nitro blocks»
(docs.arbitrum.io/run-arbitrum-node/more-types/run-classic-node); geth хранит заголовки, тела и квитанции старых блоков
во freezer отдельно от состояния (geth.ethereum.org/docs/fundamentals/databases). Узел не отдал заголовок — причина в
записи пула («узел не отдал заголовок блока N: …»), без зависания. Пул без события — в kyber.json `pools_no_log` с
причиной (состояние записано), с событием — в `pools`. Для справки (docs.arbitrum.io/run-arbitrum-node/nitro/
cli-flags-reference): `execution.rpc.log-history` — «maximum number of blocks from head where a log search index is
maintained», по умолчанию 9 400 000; значение на узле Тиграна не проверено. Потолок времени: sub_dex.py `--time-limit`
(по умолчанию 600 с) — вышел во время поиска пулов: состояние найденного пишется, причина — в block.json и README записи,
код выхода 3 (в сводке record_all — «СБОЙ sub_dex: запись неполная — …»); camelot_v2.py `--time-limit` (600 с) — поиск
границы и редких событий останавливается, в logs.json rare_search — «искали с блока X по Y, событий N», запись идёт
дальше. Потолок проверяется между запросами; один запрос к узлу — не дольше своего таймаута HTTP (120 с).
Повтор: `python3 tools/arbitrum_record/record_all.py --rpc http://127.0.0.1:8547 --date 2026-11-20 --only sub_dex`
(Windows — `py tools\arbitrum_record\record_all.py …`; нужен api.geckoterminal.com).

**Повтор по разбору пустот 20.11.2026 (решение 462):** `python3 tools/arbitrum_record/record_all.py --rpc
http://127.0.0.1:8547 --date 2026-11-20 --only kyber_elastic_v1,woofi,curve_crypto` — kyber_elastic_v1 и woofi обязательно
(записи 20.11 неполные), curve_crypto — по желанию (сырой ответ pool_count фабрики crypto v1); algebra_dir_fee повторять
не нужно. Что исправлено: woofi.py — оракул берётся с цепи (WooPP.wooracle(); 20.11 — 0xf9b0…8081, документация —
0xCf4E…34Ec, пишется рядом), базы — токены событий WooSwap пула (раньше без --base баз не было); kyber_elastic_v1.py —
версия пула по factory() (все 20 пулов GeckoTerminal — Elastic v2, фабрика 0xC7a5…8C4A), квотер и читатель тиков своей
версии (адреса v1 и v2 — шапка), пулы v1 — getPool фабрики v1 для пар GeckoTerminal, тики ещё частями (getTicksInRange
(pool, MIN_TICK, 0) у tickDistance 8 и 10 — «out of gas»), окно логов 2 000 000 блоков (2000 блоков — 0 событий);
algebra_dir_fee.py — вариант квотера, которого нет в коде (у Camelot 0x0Fc7…a76E нет кортежного), не вызывается;
curve_crypto.py — сырой pool_count и код фабрики crypto v1. Откаты get_dy двух Twocrypto — факт протокола (D < 10^17,
assert get_y), не скрипт.

Узел с короткой историей состояния (Nitro, `state-scheme=path`: состояние только последних ~128 блоков, логи старых
блоков есть) — решение 460: (1) любой ответ «historical state … is not available» — исключение `StaleState` в
`stdhttp.py` (в запись как «ошибка вызова» не попадает); (2) долгие операции (поиск логов, GeckoTerminal) — до выбора
блока B, всё чтение состояния — на свежем B после них; состояние B удалено — фаза состояния заново на новом блоке
(`recnode.fresh`, до 3 раз); (3) eth_getLogs с таймаутом узла (-32002 «request timed out») — окно пополам
(`recnode.logs_split`); (4) GeckoTerminal — пауза 12 с между запросами (меньший из официальных лимитов: «~5 calls/min»
публичного API без ключа — docs.coingecko.com/docs/keyless-public-api; «30 calls/min» — www.geckoterminal.com/dex-api;
`ARB_GT_INTERVAL` — другое значение) общая для всех скриптов запуска, при HTTP 429 — Retry-After или 15/30/60/60/60 с,
до 6 попыток (`gt.py`); (5) п. 9–10 — без скана истории в sub_dex.py, потолок времени sub_dex.py и camelot_v2.py
(раздел выше).

Узел недоступен — сводка с причиной и zip всё равно пишутся (код выхода 1). Дата папок записи одна на весь запуск
(ekubo.py и ekubo_more.py — в одну папку). Переводы строк contracts/out/ и tests/data/ git не меняет (.gitattributes,
`-text`): SHA-256 кода исполнителя на Windows совпадает с ArbExecutor.sha256.

Что запускается (адреса — `record_steps.py`, те же, что в шапках скриптов и ниже): sub_dex, kyber_elastic_v1, kyber_elastic_legacy,
saddle, woofi, pancake_stable, aero (решение 470), fluid_t1, balancer_v3, balancer_v3_buffer, curve_crypto, curve_plain,
curve_ren_wsteth (решение 481),
camelot_v2, algebra_dir_fee, lb_v20, lb_v22, ekubo_more, ekubo, balancer_v2_more, exec_v4_live.

Тесты проекта — только на реально записанных ответах (CLAUDE.md). У разработчика нет доступа к узлу Arbitrum: после
записи папки `tests/data/arbitrum_*_<дата>/` и `tests/data/executor_v4_live_<дата>/` коммитятся, с тестов снимается
`#[ignore]` (список — docs/TASKS.md, T456-03).

Каждый скрипт пишет JSON-файлы и README.md (блок, хеши SHA-256) в `tests/data/arbitrum_<семейство>_<дата>/`; адрес
узла — аргумент (`--rpc` или иной — см. шапку скрипта); адреса контрактов — из официальных источников (в шапке скрипта).

| Скрипт | Решение | Что пишет | Тест |
| --- | --- | --- | --- |
| `kyber_elastic_v1.py` (+ `kyber_elastic_ticks.py`) | 451 (T448-06), 462 | пулы KyberSwap Elastic v1 и v2 (версия — по factory() пула; pools.json — v1, pools_v2.json — v2): состояние, тики (как бот и частями), QuoterV2 своей версии, getPool фабрики, логи | `tests/arbitrum_kyber_elastic_v1_live.rs` |
| `saddle.py` | 451 (T448-07) | пулы Saddle: монеты, балансы, A, swapStorage (7 слов), calculateSwap, логи | `tests/arbitrum_saddle_live.rs` |
| `woofi.py` | 451 (T448-08), 462 | WooPPV2.2: слот EIP-1967, селекторы в коде реализации, оракул с цепи (wooracle()) и из документации — state(), базы — токены событий WooSwap, query | `tests/arbitrum_woofi_live.rs` |
| `pancake_stable.py` | 471 (T448-10) | PancakeSwap StableSwap: реестр фабрики 0x5D5f…a77a (pairLength, swapPairContract, getPairInfo, getThreePoolPairInfo), вызовы бота к пулу (coins/balances k < 8, fee, A, is_killed), рампа A, admin_fee, decimals, хвост кода (метаданные компилятора), лесенка get_dy, логи узким окном | `tests/arbitrum_pancake_stable_live.rs` |
| `sub_dex.py` (+ `sub_dex_find.py`) | 450 (T448-01…05), 460 п. 9 | KyberSwap Classic DMM / Static Fee / Legacy (пулы — хвост allPools и GeckoTerminal; событие создания — окном вокруг pool_created_at; getTradeInfo, getPools, getAmountsOut роутеров), Balancer V2 по фабрикам с версией библиотек (пулы GeckoTerminal; состояние, isPoolFromFactory, queryBatchSwap), iZiSwap (fee2pointDelta, пулы реестра pool(x, y, fee) по парам токенов), код Universal Router PancakeSwap v3 — tests/data/arbitrum_sub_dex_<дата>/; потолок времени --time-limit | `tests/arbitrum_sub_dex_live.rs` |
| `fluid_t1.py` | 440 | Fluid DEX T1 (решение 440): состояние всех пулов резолвера на одном блоке и лесенка обменов (swapIn пула → 0x…dEaD, estimateSwapIn резолвера) — для tests/arbitrum_fluid_t1.rs | `tests/arbitrum_fluid_t1.rs` |
| `balancer_v3.py` | 441 | Решения 441, 452 (T433-F2): пулы Balancer V3 всех видов — все вызовы бота к пулу (хранилище, фабрики из списков, виды пулов, крючок, прекомпайлы) и ответы Router V3 querySwapSingleTokenExactIn / ExactOut на одном блоке; тест — tests/arbitrum_balancer_v3.rs | `tests/arbitrum_balancer_v3.rs` |
| `balancer_v3_buffer.py` | 474 | Balancer V3 через буфер ERC4626: пулы фабрик (getPools) с буфером у токена, все вызовы бота к пулу и буферам, BatchRouter querySwapExactIn (буфер → пул → буфер и с буфером на одной стороне), точные previewDeposit / previewRedeem обёрток на суммах обмена, события буфера (окно `--log-blocks`) | `tests/arbitrum_balancer_v3_buffer.rs` |
| `curve_crypto.py` | 442 | Curve crypto-пулы (решения 442, 453): Twocrypto-NG (v2.1.0 и v2.0.0 — по MATH()), Tricrypto-NG, фабрика crypto v1, tricrypto 2021 — ответы на вызовы бота по calldata, проверка (get_coins, version, MATH, get_decimals), eth_getCode реализаций / математики / пулов, balanceOf и ETH пулов, decimals монет v1, лесенка get_dy — tests/data/arbitrum_curve_crypto_<дата>/ | tests/arb_curve_crypto_live.rs (NG), tests/arb_curve_crypto_v1_live.rs (v1) | `tests/arb_curve_crypto_live.rs`, `tests/arb_curve_crypto_v1_live.rs` |
| `curve_plain.py` | 472 (T450-02) | Curve StableSwap 2021 (вид curve_stableswap_plain): 2pool и plain-пулы фабрики Curve Sidechain/L2 на Plain2/3/4Basic — ответы на вызовы бота по calldata одним пакетом на блоке B (coins / balances / A_precise / fee и геттеры рампы), decimals, get_dy на долях баланса, код 2pool и реализаций (селектор exchange из 4 аргументов), логи узким окном — tests/data/arbitrum_curve_plain_<дата>/ | `tests/arbitrum_curve_plain_live.rs` |
| `curve_plain_pools.py`, `curve_sim.py` | 472 (доделка) | Помощники curve_plain.py: адреса реализаций (plain и мета), пулы фабрики, вызовы бота по (адрес, calldata); симуляция exchange — eth_call с подменой баланса и разрешения у пустого адреса одним пакетом (и для curve_crypto.py — exchange_sim) | `tests/arbitrum_curve_plain_live.rs`, `tests/arb_curve_crypto_tweak_live.rs` |
| `curve_ren_wsteth.py` | 481 (T472-10) | Curve WBTC/renBTC 0x3E01…F0Fb (вид curve_stableswap_plain) и ETH/wstETH 0x6eB2…7d80 (вид curve_stableswap_oracle) — исходники Sourcify в docs/sources/curve_arbitrum/: на свежем блоке B (recnode.fresh) ответы на вызовы бота по calldata (coins / balances / A_precise / fee / ma_exp_time, у wstETH — latestAnswer() оракула пула), геттеры рампы, oracle(), decimals, get_dy на долях баланса, симуляции exchange (вход ETH — value), код пулов и оракула — tests/data/arbitrum_curve_ren_wsteth_<дата>/ | `tests/arbitrum_curve_ren_wsteth_live.rs` |
| `camelot_v2.py` | 443 | Решение 443 (T433-F4): Camelot V2 — пары фабрики (резервы, комиссии входа, stableSwap, множители точности, getPair, код), getAmountOut пары на суммах в обе стороны на одном блоке; логи Sync, FeePercentUpdated, SetStableSwap, PairCreated. Запуск: `python3 tools/arbitrum_record/camelot_v2.py http://УЗЕЛ:8547 --factory 0x6EcCab422D763aC031210895C81787E87B43A652` (из корня репозитория) | — |
| `lb_v20.py` | 445 | Liquidity Book v2.0 (LFJ V2, вид lb_v20, решение 445) | фабрика v2.0, состояние пулов, все бины, getSwapOut роутера v2.0, слоты условия, логи; тест — tests/bsc_dex_lb_v20_arbitrum.rs | `tests/bsc_dex_lb_v20_arbitrum.rs` |
| `lb_v22.py` | 446 | Liquidity Book V2.2 — LFJ V2.2 (решение 446, T433-F7): `python3 tools/arbitrum_record/lb_v22.py http://УЗЕЛ:8547 --factory 0xb43120c4745967fa9b93E79C149E66B0f2D6Fe0c` | tests/arb_dex_lb_v22_live.rs | `tests/arb_dex_lb_v22_live.rs` |
| `ekubo.py` | 447 | Ekubo (решение 447, T433-F8): пулы GeckoTerminal ekubo-v3-arbitrum и/или `--pool`, ключи PoolKeyIndex, Core.sload (состояние, битмап, тики), MEVCapture.sload, MEVCaptureRouter.quote, логи ядра | tests/arbitrum_ekubo_live.rs | `tests/arbitrum_ekubo_live.rs` |
| `curve_crypto_rpc.py`, `curve_crypto_pools.py` | 453 | модули `curve_crypto.py` (ревью 453, файлы ≤ ~200 строк): keccak256, ABI и JSON-RPC узла; чтение пулов (вызовы бота, лесенка get_dy, код, holdings, пулы фабрик) — запускать не нужно, их импортирует `curve_crypto.py` | — |
| `ekubo_more.py` | 454 | Ekubo, дополнительная запись (после `ekubo.py`, та же папка tests/data/arbitrum_ekubo_<дата>/): PoolInitialized ядра, пулы с нативным ETH, пулы TWAMM, замер нагрузки подписки ядра | `tests/arbitrum_ekubo_more_live.rs` |
| `record_all.py`, `record_steps.py` | 459 | одна команда: все скрипты ниже по очереди, сводка, zip | — |
| `exec_v4_live.py` (+ `v4live_rpc.py`, `v4live_slots.py`, `v4live_mgr.py`, `v4live_old.py`, `v4live_cases.py`, `v4live_write.py`) | 400, 459 (T457-02) | приёмка исполнителя v4 на живых пулах: eth_simulateV1 с подменой кода (contracts/out/ArbExecutor.bin-runtime) на пустом адресе, (а) покупка / (б) продажа по видам 19 Uniswap V4 и 23 Balancer V3 (их судит правило 400; у 23 — и пулы с крючком, которые бот берёт: static / StableSurge / MEV Capture, по 2 на вид, `--bv3-per-hook`; вида нет — «нет пулов») (у Balancer V3 квотер после (а) и продажа — две симуляции: Vault.quote состояние не откатывает) (решение 478, T477-01: квотер (а) у всех видов — своя симуляция [квотер] на том же блоке H + 1, что покупка, — quoter_out; квотер на H — quoter_out_h; пулы Balancer V3 — ещё getPools() фабрик списков, `--bv3-max-pools`) и 22 Fluid, 24 LB v2.0, 1 Uniswap V3, 0 Uniswap V2 — tests/data/executor_v4_live_<дата>/ (README с SHA-256 кода и chainId — запись судит кошельки только своей сети, summary.json, hooks_reject.json, calls.json, slots.json, pools.json, raw_rpc.json) | правило — src/venues/bsc_dex/exec/executor/live_rule.rs (см. docs/TASKS.md T457-02) |
| `balancer_v2_more.py` (+ `balancer_v2_more_calls.py`) | 473 (T450-03…07) | пулы Balancer V2 фабрик composable stable v1–v6, stable phantom, meta-stable, LBP, investment, managed, linear: строки GeckoTerminal balancer_arbitrum с reserve_in_usd и фабрикой пула (порядок ликвидности по видам), на одном свежем блоке — все вызовы бота к пулу (неизменяемое и пачка блока, время блока Multicall3, getRate поставщиков) и лесенка Vault.queryBatchSwap по всем парам токенов — tests/data/arbitrum_balancer_v2_more_<дата>/; нужны recnode.py, gt.py и stdhttp.py решения 460 (в ветке t243 — копии из origin/t234/arb-merge-2 байт в байт) | `tests/arbitrum_balancer_v2_more_live.rs` |
| `stdhttp.py` | 459, 460 | HTTP стандартной библиотеки вместо пакета requests (все скрипты: `import stdhttp as requests`); StaleState — ответ узла «historical state … is not available» | — |
| `gt.py` | 460 | темп GeckoTerminal (12 с между запросами, общий для скриптов запуска) и повтор при HTTP 429 — для всех скриптов через stdhttp.get | — |
| `recnode.py` | 460 | logs_split (eth_getLogs: таймаут узла — окно пополам), fresh (фаза чтения состояния на свежем блоке, повтор на новом при StaleState); п. 9 — Deadline / TimeUp (потолок времени), block_at_time (блок по времени заголовков) | — |
| `sub_dex_find.py` | 460 п. 9 | модуль sub_dex.py: пулы без скана истории (allPools фабрик KyberSwap Classic, GeckoTerminal kyberswap_classic_arbitrum / balancer_arbitrum / пулы сети, реестр iZiSwap по парам), событие создания узким окном вокруг pool_created_at | — |
| `camelot_v2_logs.py` | 460 | модуль camelot_v2.py: нижняя граница (PairCreated первой пары фабрики или --from-block) и поиск редких событий с делением окна; п. 10 — потолок времени | — |
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

## kyber_elastic_legacy.py — KyberSwap Elastic Legacy (v1) на Arbitrum (решение 475, T462-01)

Пулы фабрики v1 `0x5F1dddbf348aC2fbe22a163e30F99F9ECE3DD50a` (в GeckoTerminal их нет — там только v2): реестр фабрики
getPool(a, b, swapFeeUnits) по парам токенов-кандидатов (GeckoTerminal — только список токенов) и уровням 8, 10, 40,
300, 1000 (конструктор фабрики, Sourcify 42161); перечисления пулов у фабрики нет. По найденным пулам — состояние, тики
частями (`--tick-chunk`, как бот с dex.elastic_ticks_chunk) и одним вызовом, ticks(t), QuoterV2 v1; события PoolCreated /
SwapFeeEnabled фабрики узким окном `--log-blocks`; потолок времени `--time-limit`. Пишет
`tests/data/arbitrum_kyber_elastic_legacy_<дата>/`, тест — `tests/arbitrum_kyber_elastic_legacy_live.rs`. Запуск:
`python3 tools/arbitrum_record/kyber_elastic_legacy.py --rpc http://УЗЕЛ:8547` (или record_all.py `--only kyber_elastic_legacy`).
Модули `gt.py`, `recnode.py`, `kyber_elastic_ticks.py`, `stdhttp.py` — те же, что в ветке t234/arb-merge-2 (байт в байт).

## aero.py (решение 470, T451-01/02)

Aero Basic (volatile/stable PoolFactory) и Aero Slipstream V3 (clFactory, крючок DynamicSwapFeeHook) — адреса
Dromos-Labs/metadex-public и metadex-slipstream-public 1.1.0 (pools-arbitrum.json, cl-arbitrum.json). Всё на одном
свежем блоке: фабрики, модули комиссии и MEV-налога, последние `--max-pools` пулов перечислением allPools(i), по пулам
Basic — metadata/POOL_TYPE/getReserves/getBaseFee/getPool и getAmountOut на доли резерва; по пулам CL — slot0, fee(),
крючок. Пишет `tests/data/arbitrum_aero_<дата>/`. Тесты — tests/arbitrum_aero_live.rs (снять #[ignore]).

Решение 470 (T470-01, T470-05, T470-06) — `aero_more.py` (вызывается из aero.py, тот же блок): параметры крючка
(secondsAgo, defaultScalingFactor, defaultFeeCap), dynamicFeeConfig и blockFee пула, observations((index + 1) % card) и
observations(0), observe([secondsAgo − δ, 0]) для δ = 0, 1, 3; fee() пула с blockOverrides {time: + δ} и {time, number:
блок + 1}; котировки Metaquoter (`0x1dDf…7868`, cl-arbitrum.json) quoteExactInputSingleV3 и виртуального квотера бота на
блоке и на + 1 / + 3 с; у пулов Basic — getAmountOutWithTotalFee и Metaquoter quoteExactInputSingleV2. Тесты —
tests/arbitrum_aero_first_fee_live.rs (снять #[ignore]). Если узел не принимает blockOverrides в eth_call — в записи
ошибка узла по этим вызовам (это факт для Тиграна, не сбой скрипта).
