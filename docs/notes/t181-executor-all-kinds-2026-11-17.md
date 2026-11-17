# t181 — исполнитель v3: все виды пулов одной транзакцией (решение 390, 17.11.2026)

Требование Тиграна: «Сделай так, чтобы мы умели все варианты». Повод — чужой арбитражник продал NFP одной транзакцией
сплитом по 5 пулам разных DEX (3 пула DODO, Uniswap V4, мост WBNB → ETH → USDT) через свой контракт.

## 1. Что было (факт кода HEAD 982e0503)

Маршрут графа идёт одной транзакцией тремя способами (quote/graph_exec.rs executable):
- **Universal Router** своей DEX (прямой путь или «умный путь» — все шаги в одном UR; pools/families.rs tradable, не форк);
- **бандл свопов** — каждая доля внутри ОДНОГО роутера, каналы отправки умеют бандл (graph_exec.rs collectable);
- **свой исполнитель ArbExecutor** — только если КАЖДЫЙ шаг маршрута — вид шага исполнителя (quote/graph_executor.rs
  all_via, pools/exec_kind.rs via_executor).

У Uniswap V4 и PancakeSwap Infinity шага исполнителя не было (pools/exec_kind.rs: `of()` → None; exec/executor/entry.rs
supported: «пул внутри менеджера исполнитель не вызывает»). Поэтому маршрут, где V4/Infinity смешаны с пулом без своего
Universal Router (DODO, Algebra, Solidly, StableSwap, Maverick, форки, …), не исполнялся НИКАК — оставался сигналом.

## 2. Таблица видов пулов: чем исполняется (до → после)

Источник: pools/families.rs spec (graphable, tradable, fork), pools/exec_kind.rs of, pools/exec_callback.rs since,
contracts/ArbExecutor.sol. «(а)» — шаг исполнителя (с какой версии контракта), «(б)» — Universal Router своей DEX,
«(в)» — свой путь (роутер форка бандлом), «(г)» — никак (только сигнал).

| dex.kind | (а) шаг исполнителя | (б) UR | (в) свой путь | до → после |
|---|---|---|---|---|
| pancakeswap_v2, uniswap_v2 | V2 (v1+) | да | — | без изменений |
| pancakeswap_v3, uniswap_v3 | V3 (v1+) | да | — | без изменений |
| **uniswap_v4** | **нет → UNI_V4 (только v3)** | да | — | UR как раньше + шаг исполнителя v3 |
| **pancakeswap_infinity_cl** | **нет → INF_CL (только v3)** | да | — | UR как раньше + шаг исполнителя v3 |
| **pancakeswap_infinity_bin** | **нет → INF_BIN (только v3)** | да | — | UR как раньше + шаг исполнителя v3 |
| **pancake_infinity_stable** | **нет → INF_CL (только v3)** — пул в крючке пула CL, обмен тот же CL по ключу | да | — | UR как раньше + шаг исполнителя v3 |
| v2_fork | V2 (v1+) | — | роутер форка бандлом | без изменений |
| v2_fork_executor | V2 (v1+) | — | — | без изменений |
| v3_fork, v3_fork_pancake, slipstream | V3 (v1+; SquadSwap — обратный вызов squadV3SwapCallback только с v2) | — | — | без изменений |
| thena_fusion, algebra_integral | ALGEBRA (v1+) | — | — | без изменений |
| solidly_v1, solidly_v2 | SOLIDLY (v1+) | — | — | без изменений |
| dodo_dvm, dodo_dpp, dodo_dsp | DODO (v1+) | — | — | без изменений |
| curve_stableswap, curve_stableswap_ng | CURVE (v1+) | — | — | без изменений |
| pancake_stableswap, lista_stableswap | STABLE_U256 / STABLE_U256_BNB (v1+) | — | — | без изменений |
| saddle_stableswap | SADDLE (v1+) | — | — | без изменений |
| maverick_v2 | MAVERICK (v1+) | — | — | без изменений |
| nomiswap_stable, metric_omm, iziswap, biswap_v3, kyber_dmm, smardex, balancer_v2, lb_v21 | NOMI_STABLE / METRIC / IZI / KYBER_DMM / SMARDEX / BALANCER / LB (с v2) | — | — | без изменений |
| kyber_elastic | в контракте ELASTIC есть (v1+), но бот вид не сопоставляет (exec_kind.rs of → None; tradable = нет, решение 121 B2) | — | — | (г) никак — без изменений (TASKS: нужна проба на пуле) |
| woofi_v2 | в контракте WOO есть (v1+), бот не сопоставляет (exec_kind.rs: шаг — только после сухой сделки на живом пуле, код закрыт) | — | — | (г) никак — без изменений (TASKS) |

**Сочетания, которые НЕ исполнялись одной транзакцией (до):**
1. V4 / Infinity (CL, Bin, Stable) + любой пул без своего UR (DODO, Algebra, Solidly, StableSwap, Curve, Maverick, форки
   V3, Slipstream, Metric, iZi, DMM, SmarDex, Balancer, LB, v2_fork_executor) — ни исполнитель (у V4 шага нет), ни UR
   (у второго пула UR нет), ни бандл (нога через не-tradable) → сигнал. **Это случай NFP (DODO + Uniswap V4).**
2. Нога маршрута через два разных роутера с V4/Infinity (например, Uniswap V4 → PancakeSwap V3 в одной доле, или Uniswap
   V4 + PancakeSwap Infinity в одной ноге) — бандл такую ногу не собирает (graph_exec.rs collectable) → сигнал.
3. V4/Infinity + другие UR — только бандлом и только если ВСЕ включённые каналы отправки бандловые; иначе сигнал.
4. Цикл DEX ↔ DEX через V4/Infinity и пул без UR — ни исполнитель, ни бандл (exec/cycle_gate.rs) → сигнал.
5. Любой маршрут через KyberSwap Elastic или WooFi — сигнал (не меняется).

**После (кошелёк с исполнителем v3):** сочетания 1–4 исполняются одной атомарной транзакцией через исполнитель
(graph_exec.rs: all_via = да, форма не UR → исполнитель; cycle_gate.rs: форма «исполнитель» предпочтительна). Остаётся
«не исполнимо»: KyberSwap Elastic, WooFi (п. 5); монета с налогом на входе в маршруте из нескольких долей или с первым
шагом не «вход переводом в пул» (graph_executor.rs tax — V4/Infinity тоже «через исполнитель»: Vault/PoolManager при
settle недосчитается налога и откатит); нативная монета BNB монетой сделки через шаг пула менеджера (ExecKind::singleton
исключён в exec/native_exec.rs и check/native_probe.rs — газ обёртки через эти шаги не мерится; как до решения 390);
пул менеджера с нативной валютой в ключе (address 0) — шаг не строится (kind_key.rs, контракт «native»).

**Кошельки v1 / v2:** всё как раньше (step_missing: «исполнитель v2 не знает шаг исполнителя UNI_V4 (вид шага есть с v3)
— переразверните исполнитель»; пулы V4/Infinity по-прежнему идут своим UR).

## 3. Что сделано

- **Контракт** contracts/ArbExecutor.sol (v3): виды UNI_V4 = 19, INF_CL = 20, INF_BIN = 21; `_singleton` (открыть
  менеджер: PoolManager.unlock / Vault.lock), обратные вызовы `unlockCallback` / `lockAcquired` → `_singletonCb` (обмен
  с точным входом, затем по ДЕЛЬТАМ исполнителя у менеджера: долг входа → sync → перевод долга → settle; кредит выхода →
  take прямо получателю шага), `_swapV4` / `_swapInf` / `_dir` / `_delta`. Интерфейсы — contracts/IPools.sol (подписи из
  официальных исходников с номерами строк). execute / executeNative / хранилище / прежние виды — без изменений.
  Код в сети 20 039 байт (EIP-170 — 24 576).
- **Версии:** contracts/out/ — v3 (развёртывает кнопка «Развернуть исполнитель»), contracts/out/v2/ — замороженная v2
  (копия артефактов HEAD 982e0503), contracts/out/v1/ — v1. code.rs: Version::V3, runtime_v2, version_of по трём;
  onchain.rs сверяет с тремя; executeNative — у v2 и v3 (Version::native).
- **Бот:** pools/exec_kind.rs — ExecKind::UniV4 / InfCl / InfBin, of(), singleton(); describe — «Universal Router своей DEX
  — как раньше; исполнитель: …»; pools/exec_callback.rs — since(вид) (v1 / v2 / v3), step_absent, step_missing по
  версии; exec/executor/callback_at.rs — то же по версии из сверки; exec/executor/kind.rs + kind_key.rs — Params::UniV4 /
  InfCl / InfBin с ключом, data = abi.encode(ключ) (то же кодирование, что PoolId — pools/key.rs), проверка «токены —
  валюты ключа, обе ERC-20»; exec/executor/entry.rs — шаг из записи реестра: ключ Entry.key сверяется со ссылкой (PoolId,
  менеджер; у Infinity Stable — крючок = адрес пула), пул шага — PoolManager (V4) или Vault (dex.vault); native_exec.rs /
  native_probe.rs — шаги менеджеров BNB не несут; интерфейс — WalletRow.svelte показывает v3.
- Проба `execute` перед отправкой (exec/executor/sim.rs, eth_simulateV1), minOut (сумма минимумов долей, call.rs),
  газ (из симуляции), сухая сделка — общие для всех видов, новых веток не потребовали.

## 4. Модель угроз шагов UNI_V4 / INF_CL / INF_BIN (контракт с деньгами)

| Угроза | Что защищает |
|---|---|
| Чужой зовёт unlockCallback / lockAcquired и заставляет исполнитель платить | Обмен только если `msg.sender == expPool` (менеджер / Vault ТЕКУЩЕГО шага, transient, ставится перед unlock/lock) и `keccak256(data) == expHash` (данные этого шага); оба стираются ДО обмена. Вне шага expPool = 0 → «pool». EVM: посторонний и владелец вне шага — «pool» |
| Крючок пула входит повторно (unlockCallback / lockAcquired с данными шага, uniswapV3SwapCallback и т. п.) | expPool / expHash уже стёрты, `_pay` пускает только expPool → откат вызова крючка; `execute` — только владелец и замок `locked`. EVM: крючок ReenterHook сделал 3 попытки — все отказаны, обмен прошёл |
| Крючок забирает с исполнителя больше суммы шага (дельта входа больше amount) | `require(0 < долг ≤ amount)` — «owed»; платится ровно долг, не баланс |
| Крючок урезает выход (комиссия дельтой) | Берётся ровно кредит исполнителя у менеджера (не выход пула); итог маршрута проверяет `received ≥ minOut` у кошелька. EVM: FeeTakingHook — получено = выход − 1,23 % |
| Произвольный вызов по адресу из данных | Вызываются только фиксированные функции интерфейсов (unlock / lock / swap / sync / settle / take / exttload / currencyDelta) по адресам из шага, который задаёт владелец (как у прежних видов); менеджер Infinity из ключа — чужой / незарегистрированный менеджер откатит Vault (onlyRegisteredApp / NoLocker) |
| Ключ не того пула, токены шага не валюты ключа, нативная валюта (address 0) | Контракт: «key» / «native»; бот — до отправки (kind_key.rs check_tokens) и PoolId ключа = id ссылки (entry.rs) |
| Недоплата / переплата менеджеру | Платится ровно долг; settle засчитывает прирост баланса менеджера — налог на перевод даст недоплату → менеджер откатит всё (CurrencyNotSettled); незакрытые дельты — откат всего unlock/lock |
| Токены застряли на исполнителе | Выход — take прямо получателю; остаток входа (пул взял меньше — предел цены) — `_sweepAll` владельцу в конце `execute` (как у прежних видов). EVM: «на исполнителе ничего не осталось» во всех сценариях |
| Обратного вызова не было (менеджер не тот) | `require(expHash == 0)` после unlock/lock — «cb»; вид UNI_V4 по адресу Vault / INF_CL по адресу PoolManager — откат (EVM) |
| Старый кошелёк v1 / v2 получает шаг 19–21 | Бот: step_missing / step_absent — путь не строится; контракт v1 / v2 — «kind» (EVM на out/v2 и out/v1) |

Где возможна потеря при ошибке шага: только при неверном minOut (итог проверяется у кошелька — недобор = откат всего) или
при ошибке расчёта выхода (котировка — та же математика V4/Infinity, что и для UR; сверка с квотером, решение 73).
Сама транзакция атомарна: ошибка любого шага — откат всего, деньги на месте, теряется только газ отклонённой транзакции
(при отправке приватным каналом откат не включается в блок — решение 41).

## 5. Чем доказано

- **EVM с официальным кодом менеджеров** (contracts/evmtest/singleton.mjs, запуск `cd contracts/evmtest && npm ci && node
  run.mjs`, 193 проверки, «ВСЕ ПРОВЕРКИ ПРОШЛИ» 17.11.2026): Uniswap/v4-core@bea7f6d1 PoolManager и FeeTakingHook,
  pancakeswap/infinity-core@3c5ef1be Vault / CLPoolManager / BinPoolManager собраны solc 0.8.26 (via_ir, как в
  foundry.toml репозиториев) из vendor-копий без изменений. Доказано: UNI_V4 / INF_CL / INF_BIN в обе стороны (получено =
  выход события Swap менеджера, взято = вход; minOut + 1 — откат), крючок с дельтой (FeeTakingHook), сплит V2 40 % +
  UNI_V4 30 % + INF_CL остаток одной транзакцией (газ 264 175), цикл USDT → UNI_V4 → COIN → INF_BIN → USDT (183 085),
  все отказы модели угроз, v2 / v1 — «kind». Газ одного шага через исполнитель (с execute): UNI_V4 ≈ 121 тыс., INF_CL ≈
  133 тыс., INF_BIN ≈ 106 тыс. (EVM, не BSC).
- **Тесты бота** (CI): tests/bsc_dex_executor_v3.rs — данные шага бота побайтно = байтам, которые контракт принял в EVM
  (запись tests/data/executor_v3_evm_2026-11-17/singleton_steps.json); ключи 37 настоящих пулов Infinity CL с крючками
  (tests/data/hook_pools_2026-11-16, ответы poolIdToPoolKey узла): PoolId = id, data шага = слова ключа из ответа
  менеджера; версии v1/v2/v3; селекторы официальных подписей в коде v3; шаг из записи реестра (PoolId, менеджер, Vault).
- **НЕ доказано:** обмен через исполнитель v3 на НАСТОЯЩИХ пулах BSC (крючки живых пулов могут проверять вызывающего —
  это покажет только симуляция на узле). Записей V4/Infinity «ZKP V4», «OPN Infinity CL» в tests/data нет (искал по
  README записей) — есть только hook_pools_2026-11-16 / hook_pools_ticks_2026-11-17 (ключи и ответы квотера). Пока нет
  симуляции на узле, шаг на живых пулах считать НЕ готовым: до неё бот всё равно не пустит маршрут без пробы `execute`
  (exec/executor/sim.rs — откат пробы = сделки нет).

### Процедура «только чтение» для серверного ИИ (eth_simulateV1 с подменой кода исполнителя)
Ничего не отправляет и не подписывает: eth_simulateV1 (execution-apis, `stateOverrides`) от адреса кошелька.
1. Взять код v3: `contracts/out/ArbExecutor.bin-runtime` ветки t181 (SHA-256 в `contracts/out/ArbExecutor.sha256`).
2. Выбрать по 2 пула каждого вида с USDT в паре и ликвидностью: Uniswap V4 (StateView/PoolManager бота), Infinity CL (с
   крючком и без), Infinity Bin, Infinity Stable — ключи из poolIdToPoolKey / poolKeys.
3. Для каждого: eth_simulateV1 на «latest», `stateOverrides`: адрес X (любой пустой) — `code` = код v3, `state`: слот 0 =
   адрес кошелька (owner), слот 1 = WBNB (wnative); у USDT — баланс кошелька (слот баланса токена) достаточный для 20 USDT
   и `allowance(кошелёк, X)`; вызовы: `X.execute(USDT, 20e18, монета, 1, срок, [шаг])` от кошелька, где шаг =
   (вид 19/20/21, пул = PoolManager / Vault, USDT, монета, 10000, abi.encode(ключ)); затем обратная сторона на полученное.
4. Записать: статус, returnData (получено), газ, логи (Swap менеджера, Transfer). Сравнить «получено» с квотером DEX того же
   блока (dex.quoter этой DEX, hookData 0x). Откат — текст причины (крючок не пускает исполнитель?).
5. Положить ответы в tests/data/executor_v3_live_<дата>/ (как прочие записи) — по ним тест «шаг проходит на настоящем пуле».

## 6. Что сделать владельцу после выкладки

1. Экран «Маппинг кошельков» → у каждого кошелька, где нужны смешанные с V4/Infinity маршруты (кошельки бирж, кошелёк
   DEX ↔ DEX, строка «Пробы и сигналы») → **«Развернуть исполнитель»** → сверить SHA-256 кода создания (показывает окно;
   это `contracts/out/ArbExecutor.sha256` строка ArbExecutor.bin) и обёртку (= WBNB) → «Подтвердить и отправить» →
   дождаться «Код в сети совпал с репозиторием … версия v3 …». Газ развёртывания по оценке EVM ≈ **4,44 млн** (v2 было
   ≈ 3,61 млн); стоимость = газ × цена газа сети в момент (окно показывает).
2. Кошелёк без переразвёртывания (v1 / v2) работает как раньше: смешанные с V4/Infinity маршруты — сигнал с причиной
   «исполнитель v2 не знает шаг исполнителя UNI_V4 … переразверните»; чистые V4/Infinity — своим UR, как раньше.
3. Новых настроек нет. Шаг INF_* берёт адрес Vault из уже обязательной `dex.vault` DEX PancakeSwap Infinity
   (pools/families.rs required).

## 7. dex.graph_exec_fallback (решение 300)

Что делает: если ЛУЧШИЙ маршрут графа неисполним, на котировке решения/исполнения повторяет сплит на подмножествах путей,
исполнимых по построению (S1 — один UR PancakeSwap, S2 — все шаги исполнителем, S3 — бандл внутри одного роутера), и даёт
лучший исполнимый (quote/graph_fit.rs). Почему не включена: решение 300 сделало её необязательной («не задавать при
первом запуске; «да» — после приёмки решения 300», docs/FIRST_RUN.md); S2/S3 на записанных данных не проверены.
После решения 390 (кошелёк v3) лучший маршрут неисполним только в редких случаях (Elastic, WooFi, налог на входе в
нескольких долях, BNB через шаг менеджера, DEX без dex.vault) — fallback почти не будет срабатывать и стоит мало. До
переразвёртывания (v1 / v2) именно он даёт сделку вместо сигнала в случае NFP: S2 без пулов V4 (DODO + PCS V3 + мост).
**Рекомендация:** включить «да» (деньги не рискуют: каждый кандидат снова проходит executable и costs; цена — до 3
доп. сплитов только на котировке решения/исполнения). Настройку не трогал — решение владельца.

## 8. Что проверить на живом боте (чтение) после выкладки и переразвёртывания
- «Проверить подключение» исполнителя: «версия v3 …; шаги Uniswap V4 и PancakeSwap Infinity … есть».
- Список DEX: у uniswap / infinity — «Universal Router своей DEX — как раньше; исполнитель: шаг UNI_V4 — исполнимо …».
- Журнал: сделки/сухие сделки с формой «граф: исполнитель» и шагами 19–21; отказы пробы `execute` с текстом причины.

## 9. Какие решения меняет
- Решение 122 (часть X/R2: «Infinity и V4 исполнитель не вызывает — только свой UR») — дополнено: у кошелька v3 —
  ещё шаги исполнителя; одиночный путь UR не меняется.
- Решение 272 (версии v1/v2, «Развернуть исполнитель» — v2): развёртывается v3; v2 заморожена в contracts/out/v2/.
