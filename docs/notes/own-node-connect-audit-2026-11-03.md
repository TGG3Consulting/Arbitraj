# Аудит: переход бота на свой узел BSC (geth-bsc v1.8.1) и отказ от KyberSwap — 2026-11-03

> Заметка Claude (зона: подключение бота к узлу и работа на нём всех вызовов). Код НЕ менялся — только разбор.
> Предложения по коду ниже — на решение Тиграна (CLAUDE.md: решение меняется — сначала запись в DECISIONS.md).

## 0. Коротко

- 🔴 **Свой узел сейчас нельзя вписать в интерфейсе.** Все адреса узла (`bsc.node_http_url`, `bsc.node_ws_url`, резервные,
  `kyber.node_http_url`, `channel.url`) — тип `Url`, а он принимает только `https`/`wss`
  (`src/settings/value.rs:58-67`). Узел слушает `http://127.0.0.1:8545` и `ws://127.0.0.1:8546` — сохранение будет отклонено
  текстом «Схема «http» не подходит». Нужна правка кода (разрешить http/ws только для 127.0.0.1/::1/localhost) или TLS-прокси перед узлом.
- 🔴 **Нет проверки «узел синхронизирован и не отстаёт».** Свежесть = «номер блока вырос за `feed.stale_after_ms`»
  (`node/heads.rs:83-99, 160-169`). Догоняющий узел растит номер каждую секунду — бот считает его свежим и торгует на старом
  состоянии. `eth_syncing`, возраст блока (`timestamp` против часов сервера) и сравнение с внешней головой не проверяются нигде
  (ни в работе, ни в «Проверить подключение»).
- 🔴 (если `dex.pools_per_sub` > 1000) **Подписки пулов CL/V4 по PoolId упадут на своём узле.** В geth-bsc v1.8.1 жёстко
  `maxSubTopics = 1000` вариантов в одной позиции topics (`eth/filters/api.go:64-66`, проверка `:860-861`), настройкой узла не
  меняется. Решение 79 советовало «крупный» `dex.pools_per_sub` (на publicnode прошло 5000). Лечится настройкой: `dex.pools_per_sub = 1000`.
- 🟠 Пока `exec.via_kyber = да`, каждый своп уходит **в публичный мемпул** (`kyberswap/node.rs:166-168`, адрес
  `kyber.node_http_url`) без темпа и без резерва — открыт сэндвичам на весь `kyber.slippage_bps`. Переход на свой
  сборщик снимает это, только если включены **закрытые** каналы (48 Club и др.) и выключен `public`.
- 🟠 Резерв узла переключается только при сетевой ошибке и отдельно для HTTP и WS (`failover.rs:239-244`) — свой узел
  (HTTP) + publicnode (WS), или наоборот, дадут чтение состояния на номере блока «чужого» узла; отставание своего узла
  переключения не вызывает.
- 🟠 Бот и узел на одном сервере: всплеск CPU бота (решение 109: 1470% при MEXC) отнимет процессор у импорта блоков —
  узел отстанет, а бот этого не увидит (см. 🔴 выше).
- 🟠 `bsc.node_http_rps` подбирался под публичный узел (в решении 113 — 20 вызовов/с, отсюда 11 с на балансы). Для своего
  узла — поднять (совет: 500 на старт), иначе выигрыш от своего узла теряется в очереди бота.

Всё остальное, что бот вызывает, свой узел с этими флагами поддерживает (таблица ниже): нужные методы — в namespace
`eth`; `txpool`, `debug`, `admin` боту **не нужны** — включать не надо.

---

## 1. Что проверено и по каким источникам

- Код бота: `src/venues/bsc_dex/node/*`, `send/*`, `wallet/*`, `check/simulate.rs`, `connection*.rs`, `pools/{scan,
  history_v4, deploy_block, group, created, listen_log, market}.rs`, `quote/{gas,warm}.rs`, `src/venues/kyberswap/{node,tx}.rs`,
  `src/settings/value.rs`, `src/settings/catalog/{dex,send,kyber,base}.rs`, решения 41, 45, 57, 61, 68, 77, 79, 83, 91, 92, 95, 108, 109, 112, 113.
- Узел: исходники **bnb-chain/bsc тег v1.8.1** (`git clone --branch v1.8.1`, коммит `947ba1cb1ef05d4a5f71b738113a3d8b4c16f6b7`,
  `version/version.go`: 1.8.1 — тот же коммит, на который уже ссылается код бота). Ниже ссылки вида `bsc:файл:строка` — на этот коммит.
- Что не удалось проверить по исходникам — помечено «проверить вживую» (раздел 7).

---

## 2. Таблица JSON-RPC методов бота

Узел: HTTP и WS — namespaces `eth, net, web3`. Все методы `eth_*` бота зарегистрированы в namespace `eth`
(`bsc:internal/ethapi/backend.go:141-162`, фильтры — `bsc:cmd/utils/flags.go:2607-2615`).

| Метод | Где вызывается (файл:строка) | Параметры | Поддержка в geth-bsc v1.8.1 с этими флагами | Риск | Что сделать |
|---|---|---|---|---|---|
| `eth_blockNumber` | `node/heads.rs:185` (опрос, мимо очереди), `connection.rs:84`, `connection_extra.rs:54`, `pools/market.rs:58`, `pools/listen_log.rs:163`, `pools/history_v4.rs:88`, `pools/deploy_block.rs:55`, `wallet/transfer_status.rs:144`, `node/receipts_fetch.rs:50` (в пачке), `pools/group.rs:106` (по WS), `kyberswap/node.rs:115` | — | Да (`bsc:internal/ethapi/api.go:331`) | Во время догона растёт → бот считает голову свежей (🔴 П2) | Проверка синхронизации и возраста блока (П2) |
| `eth_chainId` | `connection.rs:80, 94`, `connection_extra.rs:52, 67`, `kyberswap/connection.rs:80` | — | Да (`api.go:326`) → `0x38` | — | — |
| `eth_getBlockByNumber` | `node/heads.rs:141` (время блока, раз на блок), `node/receipts_fetch.rs:48`, `node/receipts_io.rs:59` | номер в hex, `false` | Да (`api.go:516`) | — | — |
| `eth_getBlockReceipts` | `node/receipts_fetch.rs:49` (только при `bsc.pool_events_source = receipts`) | номер в hex | Да (`api.go:786`) | Блоки старше хвоста (`history.blocks=600000` ≈ 3 сут. при ~2,2 блока/с) — квитанций нет; лента читает только свежие | — |
| `eth_call` | `node/call.rs:33, 75, 121`, `node/multicall.rs:94` (Multicall3), `quote/quoter.rs:33`, `kyberswap/node.rs:100` | `"latest"` или номер блока (`load.rs:51`, `load_batch.rs:69`, `listen_log.rs:148`); Multicall3 частями `dex.load_batch` | Да (`api.go:1120`); газ до `RPCGasCap=100M`, время до `RPCEVMTimeout=30s` | Номер блока старше ~128 блоков (≈57 с) — у PBSS full-узла состояния нет: «state 0x… is not available» (`bsc:triedb/pathdb/reader.go:187-191`, `TriesInMemory: 128` — `eth/ethconfig/config.go:72`) | На своём узле очередь короткая — ок; текст ошибки — 🟡 П12 |
| `eth_simulateV1` | `check/simulate.rs:110`, `connection.rs:123` | `{blockStateCalls:[{calls}], validation:false}`, номер блока (проба) / `"latest"` (проверка) | **Есть** (`api.go:1142`); пределы: 256 блоков, 5000 вызовов в блоке, 10000 всего (`internal/ethapi/simulate.go:43-58`); время симулированного блока = базовое + 12 с (`simulate.go:56-57, 577-583`) | Вызовов у проб единицы–десятки — в пределах | `dex.deadline_s` > 12 с (решение 61 в силе) |
| `eth_estimateGas` | `send/tx.rs:96`, `kyberswap/node.rs:140` | без блока → `latest` | Да (`api.go:1243-1248`, по умолчанию latest) | — | — |
| `eth_gasPrice` | `quote/gas.rs:88` (раз на блок, `quote/warm.rs`), `kyberswap/node.rs:121` | — | Да (`api.go:88`) — оракул своего узла (`FullNodeGPO`: 20 блоков, 60-й перцентиль, `eth/ethconfig/config.go:45-53`) | — | — |
| `eth_feeHistory`, `eth_maxPriorityFeePerGas` | **Не вызываются** (транзакции legacy EIP-155, `send/tx.rs:108-135`) | — | Есть (`api.go:100, 118`) | — | — |
| `eth_getBalance` | `wallet/balances.rs:35`, `wallet/events_native.rs:82`, `kyberswap/node.rs:162` | `"latest"`; у events_native — блок «голова − `send.deposit_confirmations`» | Да (`api.go:339`) | `send.deposit_confirmations` ≥ ~128 — состояния нет | Держать подтверждения < 100 |
| `eth_getTransactionCount` | `wallet/nonce.rs:155`, `send/track.rs:88`, `send/settle.rs:60`, `wallet/deposit.rs:72`, `wallet/events_native.rs:83`, `kyberswap/node.rs:156` | `"latest"` (pending нигде) | Да (`api.go:2037`) | — (pending не нужен: свои транзакции идут в закрытые каналы, в мемпуле своего узла их нет) | — |
| `eth_getTransactionReceipt` | `send/receipt.rs:49`, `wallet/transfer_status.rs:70`, `kyberswap/node.rs:172` | хеш | Да (`api.go:2212`) | Пока идёт индексация транзакций — ошибка `-32000 "transaction indexing is in progress"` вместо null (`api.go:2215-2218`, `errors.go:70-89`) | Бот такие ошибки пишет предупреждением и ждёт дальше (`send/track.rs`) — ок; торговлю не включать до конца индексации (раздел 7) |
| `eth_getTransactionByHash` | `send/receipt.rs:67`, `wallet/events_tx.rs:42`, `wallet/native_status.rs:60` | хеш | Да (`api.go:2056`, тот же TxIndexingError) | как выше | — |
| `eth_getCode` | `connection_extra.rs:79, 97` (`latest`), `pools/deploy_block.rs:109` (бинарный поиск по **старым** блокам) | адрес, блок | `latest` — да (`api.go:725`). Старые блоки — **нет**: исторического состояния у PBSS без archive нет (`bsc:core/blockchain.go:329` — индекс истории состояния только при `ArchiveMode`) | Поиск блока развёртывания менеджера V4 и проверка «История состояния» упадут | `dex.history_scan = нет` (решение 58 разрешает) |
| `eth_getLogs` | `pools/scan.rs:81` (история Initialize V4, только при `dex.history_scan = да`) | `{address, topics, fromBlock, toBlock}` кусками `dex.logs_block_batch` | Метод есть (`eth/filters/api.go:604`). Диапазон больше 5000 блоков — отказ «exceed maximum block range 5000» (`RangeLimit: 5000` — `eth/ethconfig/config.go:85`, проверка `filter.go:148-163`). Блоки вне `history.logs=600000` — поиск без индекса (медленно, `filter.go:443-470`). Блоки ниже хвоста фризера (снапшот `pruneancient`, хвост 125973687) и старше `history.blocks=600000` — ошибка «header not found» / «failed to get logs for block #N» (`filter.go:458-461`, `filter_system.go:111-117`). `PrunedHistoryError` в v1.8.1 не возникает: обрезка истории в режиме KeepAll выключена (`core/blockchain.go:907-912`, `HistoryPruningCutoff` = 0) | История V4 «от блока развёртывания» на этом узле невозможна | `dex.history_scan = нет`; если когда-нибудь да — `dex.logs_block_batch` ≤ 5000 |
| `eth_subscribe` `logs` | `node/ws_task.rs:118` ← `pools/group.rs:82` (пулы), `pools/created.rs:100` (новые пулы), `wallet/events.rs:96` (Transfer кошелька) | `by_address` (`node/subs.rs:237-239`), `by_pool_id` с PoolId в `topics[1]` (`subs.rs:242-244`); размер — `dex.pools_per_sub` | Есть (`eth/filters/api.go:412`). Пределы: не больше 4 позиций topics и **не больше 1000 вариантов в одной позиции** — жёстко в коде (`api.go:64-66`, `:860-861`), флагом не меняется; адресов — не больше `LogQueryLimit` (`filter_system.go:321-333`), а он в v1.8.1 **по умолчанию 0 = без предела** (в `Defaults` его нет, `eth/ethconfig/config.go:56-86`; флаг `--rpc.logquerylimit` — `cmd/utils/flags.go:783-787`) | `by_pool_id` > 1000 PoolId → «exceed max topics» → пулы вида не слушаются (🔴 П3) | `dex.pools_per_sub = 1000`; в config.toml проверить, что нет `LogQueryLimit` < 1000 |
| `eth_subscribe` `newHeads` | `connection.rs:96` (только «Проверить подключение») | — | Есть (`api.go:321`) | Проходит и у догоняющего узла | Добавить проверку синхронизации (П2) |
| `eth_unsubscribe` | `node/ws_task.rs:144`, `pools/group.rs:99`; `node/receipts_handle.rs:88` — имитация внутри бота, к узлу не идёт | id | Есть | — | — |
| `eth_sendRawTransaction` | `send/ready.rs:130` (канал с `Single::Raw` — `public`, по `channel.url`), `kyberswap/node.rs:167` (по `kyber.node_http_url`) | подписанная legacy EIP-155 | Есть (`api.go:2414`); только EIP-155 (`api.go:2321-2324`), предел комиссии `RPCTxFeeCap` = 1 BNB (`config.go:81`) | Через свой узел — в свой мемпул и p2p, видно всем (🟠 П4) | Свопы — только закрытые каналы |
| `eth_sendPrivateTransaction`, `eth_sendBundle`, `bsc_private_tx`, `blxr_submit_bundle` | `send/ready.rs:133, 136, 139, 143` | — | **Не к своему узлу** — только к каналам билдеров по их `channel.url` | — | — |
| `eth_syncing`, `net_peerCount`, `web3_clientVersion` | **Не вызываются** | — | Есть (`api.go:166`; net/web3 включены) | Нечем проверить синхронизацию (🔴 П2) | См. П2 и раздел 7 |
| `txpool_*`, `debug_*`, `admin_*` | **Не вызываются** | — | Выключены на HTTP/WS | — | Не включать. Для ручной диагностики есть IPC (`geth.ipc` в datadir) — там все namespaces |

JSON-RPC пачки (batch): `node/rpc.rs:68-104` (пачка eth_call без Multicall3), `node/receipts_fetch.rs` (3 вызова). Предел узла
`BatchRequestLimit=2000` (по умолчанию 1000 — `node/defaults.go:71`) и `BatchResponseMaxSize=100MB` — с запасом.

Заголовок Host: бот ходит по IP — geth пропускает IP без проверки vhosts (`bsc:node/rpcstack.go:456-471`). Если будет
TLS-прокси с именем хоста — это имя нужно в `--http.vhosts`, иначе 403 «invalid host specified» (`rpcstack.go:481`).

---

## 3. Проблемы

### 🔴 П1. Адрес своего узла нельзя сохранить (только https/wss)
- Доказательство: `src/settings/value.rs:58-67` — `Kind::Url` принимает только `"https" | "wss"`, иначе «Схема «{other}» не
  подходит». Типа `Url` все адреса узла: `settings/catalog/dex.rs:56` (`bsc.node_ws_url`), `:60-61` (резервные), `:63`
  (`bsc.node_http_url`), `catalog/kyber.rs:34` (`kyber.node_http_url`), `catalog/send.rs:29` (`channel.url`).
  Плюс отдельно `node/receipts_nodes.rs:46-49` (`bsc.receipts_node_urls` — только `https://`).
- Узел по флагам — `http://127.0.0.1:8545`, `ws://127.0.0.1:8546`. Решение 92 само пишет: «свой geth по https/wss — позже».
- Варианты (на решение Тиграна):
  1. **Код (совет):** в `value.rs` для `Kind::Url` разрешить `http`/`ws`, только если хост — loopback (`127.0.0.0/8`, `::1`,
     `localhost`). Довод «в адресе может быть ключ» не нарушается: трафик не выходит с сервера. Одна точечная правка +
     запись в DECISIONS.md + docs/SETTINGS.md. `tokio-tungstenite` и `reqwest` открытые `ws://`/`http://` поддерживают.
  2. Без кода: TLS-прокси (nginx/caddy) на 127.0.0.1 с самоподписанным CA, добавленным в системное хранилище
     (`update-ca-certificates`; reqwest 0.13 у нас с `rustls-platform-verifier`, tungstenite — `rustls-tls-native-roots`,
     оба читают системные корни) + имя хоста в `/etc/hosts` и в `--http.vhosts`. Лишнее звено и лишние доли миллисекунды.

### 🔴 П2. Нет проверки синхронизации и отставания узла («не торгуем вслепую»)
- Доказательство: `node/heads.rs:83-99` — голова свежая, если номер вырос не раньше `feed.stale_after_ms` назад;
  `heads.rs:160-169` — `observe` только сравнивает номера. Время блока (`timestamp`) читается (`heads.rs:138-156`), но с
  часами сервера не сравнивается. Поиск `eth_syncing`/`block_age`/сравнения с внешней головой по `src` — ничего.
  `connection.rs:79-88` проверяет только chainId и номер блока; WS-проверка (`:91-116`) — только что newHeads прислал блок.
- Что будет: пока узел догоняет сеть после снапшота (или отстаёт из-за пиров/CPU), `eth_blockNumber` растёт каждую
  секунду → `fresh()` = Ok → котировки, пробы, `eth_call "latest"` идут на **старом** состоянии; срок сделки считается
  от времени старого блока (`exec/trade.rs:152`) — транзакция в сети откатится по сроку (газ потерян), а сторона на бирже
  может уже исполниться по ложному сигналу. Узел встал совсем — номер не растёт, `StaleData` — это работает.
- Что сделать (код, на решение Тиграна; настройки — новые, без значений по умолчанию):
  - в опросе головы (`heads.rs`) или прогреве (`quote/warm.rs`, уже читает время каждого блока) — «возраст головы» =
    часы сервера − `timestamp` блока; больше новой настройки (например `bsc.head_max_age_ms`) → `StaleData`
    (BSC даёт ещё миллисекунды в заголовке, но секундной точности хватает для «отстал на 2+ с»);
  - `eth_syncing` ≠ `false` → `StaleData` и запись в журнал (метод в `eth`, `api.go:166-200`);
  - по желанию — сравнение с внешней головой (резервный узел) раз в N секунд, разница больше `bsc.max_head_lag_blocks` → `StaleData`;
  - то же — в «Проверить подключение» (П9).
- До правки: **не включать торговлю, пока узел не догнал** — проверять руками (раздел 7, пункты 2–4).

### 🔴 П3. `dex.pools_per_sub` больше 1000 — подписки по PoolId (CL/V4) отвергнутся
- Доказательство: `bsc:eth/filters/api.go:64-66` (`maxSubTopics = 1000`), `:860-861` (`len(topic) > maxSubTopics` → `errExceedMaxTopics`)
  — в разборе `FilterCriteria`, общем для `eth_getLogs` и `eth_subscribe logs` (`api.go:412`, `:780`). Флагом не меняется.
  У бота `by_pool_id` кладёт все PoolId пачки в `topics[1]` (`node/subs.rs:242-244`, `pools/group.rs:171-177`), размер пачки —
  `dex.pools_per_sub` (`subs.rs:225-234`). Решение 79 и подсказка настройки (`catalog/dex.rs:97`) советуют «крупным», чтобы
  весь вид шёл одной подпиской (на publicnode прошло 5000).
- Что будет на своём узле: подписка менеджера CL/V4 с > 1000 пулами — ответ «exceed max topics», пулы этого вида не слушаются
  (недостоверны, сделок по ним нет). `by_address` (V2/V3) пройдёт: `LogQueryLimit` по умолчанию 0 = без предела
  (`filter_system.go:325`, в `Defaults` поля нет).
- Что сделать: настройка `dex.pools_per_sub = 1000` (без кода). Соединений станет `⌈пулов вида / 1000⌉` на вид — единицы.
  Подсказку настройки и решение 79 стоит дополнить строкой про предел geth (на решение Тиграна).

### 🟠 П4. Своп через KyberSwap уходит в публичный мемпул, без темпа и резерва
- Доказательство: `venue.rs:93-98` — при `exec.via_kyber` своп делает `kyberswap::trade`; отправка — `kyberswap/node.rs:166-168`
  `eth_sendRawTransaction` на `kyber.node_http_url` (сейчас публичный узел). `kyberswap/node.rs:58-82` — прямой POST без
  очереди `bsc.node_http_rps` и без резервного узла. Закрытые каналы (`send/*`, решение 41) этот путь не использует.
- Риск: своп виден до блока → сэндвич на всю ширину `kyber.slippage_bps`.
- Отказ от KyberSwap (`exec.via_kyber = нет`) — свопы идут собственным сборщиком через `send::select` (`send/mod.rs:51-76`):
  нужен хотя бы один готовый канал, открытый `public` — только один и не вместе с закрытыми. Перед переключением проверить
  на экране «Каналы отправки»: `public` выключен, закрытые (48 Club, BlockRazor, MEV Guard, …) включены и «готовы».

### 🟠 П5. Резервный узел: переключение только по сети и отдельно для HTTP и WS; отставание не переключает
- Доказательство: `node/failover.rs:239-244` — на резервный уходят только `Unreachable | Timeout | RateLimited`; ошибки JSON-RPC
  («header not found», «state … is not available») и отставание — нет. HTTP (`rpc.rs:108-130`) и WS (`ws.rs:55-77`) переключаются
  независимо. Пачки пулов читают состояние на номере блока своего WS-соединения (`pools/group.rs:104-108`) по HTTP.
- Риск: WS на одном узле, HTTP на другом — номер блока одного узла, чтение у другого (ровно болезнь publicnode из решения
  108). Резерв = publicnode возвращает «nonce too low / старые балансы» на время `bsc.node_backup_hold_s`.
- Совет: на время приёмки резервные адреса **оставить пустыми** — сбой своего узла = пауза торговли (видно сразу, «не торгуем
  вслепую»). Резерв решать после недели работы; если ставить — HTTP и WS одного и того же провайдера.

### 🟠 П6. Бот и узел делят процессор
- Решение 109: всплеск бота до ~1470% CPU (≈15 ядер из 24) и загрузка 25.9. На этом же сервере импорт блоков geth
  замедлится → узел отстанет → без П2 бот этого не заметит.
- Что сделать (сервер, не код): приоритет узлу — `systemctl edit bsc-node` → `CPUWeight=1000` (боту — 100) или
  `Nice=10` / `CPUQuota=` у службы бота; следить за отставанием (раздел 7, п. 3) при включённых всех биржах.

### 🟠 П7. Темп `bsc.node_http_rps` подобран под публичный узел
- Решение 113: при `bsc.node_http_rps = 20` балансы шли ~11 с, путь сделки 24–28 с. Очередь (`node/pace.rs:353-374`,
  `node/queue.rs`) и классы срочности (решение 61) своему узлу нужны — как порядок «сделка → проверки → котировки → фон» и
  как предохранитель импорта блоков, но не как лимит провайдера.
- Совет: 500 на старт; при ровном отставании узла ≤ 1 блока — 1000–2000. 0 нельзя (`pace.rs:358-361` — ошибка).

### 🟡 П8. История V4 и «История состояния» на pruned PBSS-узле невозможны
- `pools/deploy_block.rs:87-118` ищет блок развёртывания бинарным поиском `eth_getCode` по старым блокам — у full-узла PBSS
  истории состояния нет (`core/blockchain.go:329`, только archive). `eth_getLogs` старше хвоста — ошибка (таблица выше).
  `connection.rs:136-181` при `dex.history_scan = да` покажет «нужен архивный узел».
- Что сделать: `dex.history_scan = нет` (решение 58: пулы V4/Infinity — из справочника). Если в SQLite история уже прочитана
  до блока в пределах хвоста узла — дочитывание пойдёт; разрыв больше ~3 суток — та же ошибка.

### 🟡 П9. «Проверить подключение» не проверяет главное для своего узла
- Сейчас (`connection.rs:34-76`): chainId 56, номер блока, WS + newHeads, eth_simulateV1, Multicall3, обёртка BNB,
  индексатор, история V4 (если включена), кошелёк, резервные.
- Не хватает: `eth_syncing = false`; возраст головы (время блока против часов сервера); число пиров (`net_peerCount`, namespace
  `net` включён); пробная подписка `logs` с реальным размером пачки (`dex.pools_per_sub` PoolId) — П3 сейчас вылезет только в
  работе; при `receipts` — один `eth_getBlockReceipts`. Добавить вместе с П2 (код, на решение Тиграна).

### 🟡 П10. Опрос блоков вместо newHeads — причина (цена NodeReal) на своём узле ушла
- Решение 57 выбрало опрос из-за CU NodeReal. На своём узле поток newHeads бесплатный и приходит сразу после импорта; опрос
  раз в `bsc.blocks_poll_ms` добавляет в среднем половину интервала. Не обязательно: при `bsc.blocks_poll_ms = 50–100` задержка
  мала, а локальный `eth_blockNumber` стоит ~доли миллисекунды. Переделка — по желанию, отдельным решением.

### 🟡 П11. Ошибка «transaction indexing is in progress»
- После старта узел строит индекс транзакций на `history.transactions=600000`; пока строит, на неизвестный хеш
  `eth_getTransactionReceipt`/`ByHash` отвечают ошибкой `-32000`, а не `null` (`api.go:2065-2067, 2215-2218`). Бот пишет
  предупреждение и ждёт (`send/track.rs:60-82`) — работает, но торговлю включать после окончания индексации
  (`eth_syncing` = `false`; во время работы видно поле `txIndexRemainingBlocks`).

### 🟡 П12. Текст «нет состояния блока» у PBSS другой, чем ждёт бот
- `venues/bsc_dex/errors.rs:50-58` переводит в `StaleData` («чтение на закреплённом блоке опоздало») только «missing trie node».
  У PBSS v1.8.1 текст — «state 0x… is not available» (`triedb/pathdb/reader.go:187-191`) → у бота это `BadResponse`.
  Влияние малое (на своём узле чтение на блоке старше ~57 с редкость), но причина в журнале будет непонятной. Проверить
  вживую (раздел 7, п. 11) и при подтверждении добавить текст в `errors.rs` (код).

### 🟡 П13. Системный прокси HTTP-клиента
- `reqwest` собран с `system-proxy` (`Cargo.toml:21`). Если у процесса бота заданы `HTTP_PROXY`/`HTTPS_PROXY`, запросы к
  `127.0.0.1` могут уйти в прокси. Проверить окружение службы бота (раздел 7, п. 15); при наличии — `NO_PROXY=127.0.0.1,localhost`.

---

## 4. Ответы на вопросы задачи

**(1) Переключение между узлами.** Основной → резервный по сетевой ошибке/таймауту/429, удержание `bsc.node_backup_hold_s`,
HTTP и WS отдельно (`failover.rs`). Проверки «узел отстаёт / не синхронизирован» нет ни в работе, ни в проверке подключения
(🔴 П2). Узел встал совсем — голова стареет, `StaleData`, сделок нет. Узел догоняет или отстаёт на N блоков — бот торгует.

**(2) Темп и очередь.** Нужны как порядок срочности и предохранитель импорта узла, не как лимит провайдера. Значения — раздел 5.
`bsc.blocks_poll_ms` идёт мимо очереди (`rpc.rs:54-63`) — на своём узле можно 50–100 мс.

**(3) Нагрузка на свой узел** (оценка по коду; BSC ≈ 2,2 блока/с — решение 68; ~200 монет, ~3000 пулов):
- Источник `ws` (совет):
  - подписки: `⌈пулов вида / dex.pools_per_sub⌉` на вид + «новые пулы» + Transfer кошелька — при 1000 порядка 6–12 подписок;
    уведомлений — логи наших пулов в блоке × 2,2/с (в блоке BSC 470–870 логов всего, решение 68) — сотни в секунду, для
    локального geth пренебрежимо;
  - HTTP постоянно: `eth_blockNumber` 1000/`bsc.blocks_poll_ms` (при 100 мс — 10/с); прогрев `eth_getBlockByNumber` +
    `eth_gasPrice` на каждый блок ≈ 4,4/с (`quote/warm.rs`); бины Infinity Bin — eth_call на событие бина
    (`pools/listen_log.rs:143-158`); квитанция своей транзакции — на каждую голову ≈ 2,2/с на транзакцию в полёте; сверка
    балансов — один Multicall3 за круг (решение 113); фоновые циклы (квотер, крючки, налоги) — Multicall3/eth_simulateV1 по
    их настройкам. Итого обычно десятки вызовов в секунду;
  - пик — старт: загрузка ~3000 пулов Multicall3-пачками `dex.load_batch` (группы по половине пачки) + тики CL + первый
    проход проб налогов ~200 монет. На своём узле — секунды–десятки секунд при `bsc.node_http_rps` 500+.
- Источник `receipts`: +3 вызова на блок (заголовок, квитанции, голова) ≈ 6,6/с и 0,4–0,7 МБ на блок ≈ 1–1,5 МБ/с по loopback.
  Свой узел потянет, но `ws` проще и быстрее (уведомление сразу после импорта, без опроса и машинерии решения 77).

**(4) Отправка.** Сейчас: свопы при `exec.via_kyber = да` — открыто через `kyber.node_http_url` (🟠 П4); переводы кошелька и
собственный сборщик — через каналы `send/*` (решение 41). Свой узел для отправки свопов **не использовать**:
`eth_sendRawTransaction` в свой узел = свой мемпул → p2p → видят все поисковики сэндвичей. Защита — закрытые каналы билдеров
(48 Club, BlockRazor, MEV Guard, Blink, NodeReal, bloXroute — решения 33, 41), несколько закрытых параллельно, `public`
выключен. Свой узел нужен для чтения, оценки газа, проб и квитанций — там он и даёт выигрыш (один узел = нет «nonce too low»
и старых балансов из решений 108, 112). Канал `public` на свой узел и так нельзя вписать без П1.

**(5) «Проверить подключение».** Проверяет chainId 56 (`connection.rs:193-202`), номер блока, WS + newHeads, eth_simulateV1,
Multicall3, кошелёк. Для своего узла не хватает синхронизации, возраста головы, пиров и пробной подписки реального размера (🟡 П9).

**(6) Что зашито под публичный узел.**

| Что | Где | Свой узел |
|---|---|---|
| nonce = max(узел, запомненный) (решение 112) | `kyberswap/tx.rs`, `wallet/nonce.rs:151-155` | **Оставить** — дёшево, страхует при перезапуске узла |
| Ожидание полного баланса перед перегоном USDT (решение 108) | `core/usdt_relay_retry.rs` | **Оставить** — нужно и для биржевой стороны; на своём узле ожидание кончится сразу |
| Балансы одним Multicall3 (решение 113) | `wallet/balances_mc.rs` | **Оставить** — быстрее и на своём узле |
| Очередь со срочностью (решение 61) | `node/pace.rs`, `node/queue.rs` | **Оставить**, поднять `bsc.node_http_rps` |
| Резерв + удержание (failover) | `node/failover.rs` | Оставить код; адреса резерва — см. П5 |
| Лента квитанций: раздача по списку публичных узлов, гонка, двойная сверка «ложного реорга», голова узла в пачке (решения 71, 72, 77) | `node/receipts_*.rs`, `rpc_race.rs` | **Не нужна** при `ws`; `bsc.receipts_node_urls` — пусто |
| Опрос блоков вместо newHeads (решение 57) | `node/heads.rs` | Работает; переделка по желанию (П10) |
| Отдельный узел исполнения KyberSwap | `kyberswap/node.rs`, `kyber.node_http_url` | **Лишнее** при `exec.via_kyber = нет` |
| «missing trie node» → StaleData | `errors.rs:50-58` | Дополнить текстом PBSS (П12) |

---

## 5. Настройки для своего узла (что вписать в интерфейсе)

| Настройка | Значение | Почему |
|---|---|---|
| `bsc.node_http_url` | `http://127.0.0.1:8545` | После П1 (или https-адрес TLS-прокси) |
| `bsc.node_ws_url` | `ws://127.0.0.1:8546` | После П1 |
| `bsc.node_http_url_backup`, `bsc.node_ws_url_backup` | пусто на время приёмки | П5 |
| `bsc.node_backup_hold_s` | не задавать, пока нет резерва | — |
| `bsc.pool_events_source` | `ws` | Раздел 4 (3) |
| `dex.pools_per_sub` | `1000` | 🔴 П3, предел geth |
| `bsc.node_http_rps` | `500` на старт, потом 1000–2000 | 🟠 П7 |
| `bsc.node_background_share_pct` | как сейчас | Решение 61 |
| `bsc.blocks_poll_ms` | `100` (можно 50) | Локально дёшево; строго меньше `feed.stale_after_ms` (`heads.rs:207-219`) |
| `feed.stale_after_ms` [bsc_dex] | как сейчас, не меньше ~2000 | Блок ~0,45 с; запас на редкие медленные блоки. От отставания при догоне НЕ защищает (П2) |
| `bsc.receipts_node_urls` | пусто | Лента не нужна при `ws` |
| `dex.history_scan` | `нет` | 🟡 П8 |
| `dex.logs_block_batch` | ≤ 5000 (если история когда-нибудь включится) | `RangeLimit` 5000 |
| `send.deposit_confirmations` | как сейчас, но < 100 | Состояние у PBSS — ~128 последних блоков |
| `exec.via_kyber` | `нет` — только после проверки каналов | 🟠 П4 |
| Каналы: `channel.enabled [public]` | `нет` | Свопы — только закрытые |
| Каналы: закрытые (48 Club и др.) | включены, «готовы» | `send/mod.rs:51-76` |
| `http.timeout_ms`, `http.total_ms`, `http.body_idle_ms`, `http.max_body_bytes` | как сейчас | Локальный узел быстрее; самые тяжёлые Multicall3 должны укладываться |
| `venue.ws_ping_ms`, `venue.ws_silence_ms` [bsc_dex] | как сейчас | — |
| `kyber.node_http_url` | можно не трогать | Не используется при `exec.via_kyber = нет` |

---

## 6. Флаги и config.toml узла

- **Менять не нужно:** `txpool`, `debug`, `admin` на HTTP/WS не включать — бот их не вызывает. Для ручной диагностики есть IPC
  (`geth-bsc attach /data/bsc/node/geth.ipc`, там все namespaces, наружу не торчит).
- `RPCGasCap=100M`, `RPCEVMTimeout=30s`, `BatchRequestLimit=2000`, `BatchResponseMaxSize=100MB` — с запасом для Multicall3 и проб.
- Проверить в config.toml, что нет `LogQueryLimit` меньше 1000 (по умолчанию 0 = без предела). Предел 1000 PoolId в `topics`
  флагом не снимается — только `dex.pools_per_sub` (П3).
- `--rpc.rangelimit` (5000) — не трогать: `eth_getLogs` боту не нужен при `dex.history_scan = нет`.
- `--history.logs=600000` — боту не нужен (подписка `logs` от индекса логов не зависит). Можно оставить; `--history.logs.disable`
  сэкономит диск и CPU — по желанию.
- `--history.blocks=600000` — минимум (`FullImmutabilityThreshold` = 600000, `params/network_params.go:32`; меньше geth сам поднимет
  до 600000 — `cmd/utils/flags.go:2248-2253`). Боту хватает.
- Приоритет CPU узлу — П6 (systemd, не флаг geth).

---

## 7. Проверка вживую на сервере (команды для ИИ на сервере)

Обёртка (без jq):

```bash
R(){ curl -s -m 10 -X POST -H 'Content-Type: application/json' --data "$1" http://127.0.0.1:8545; echo; }
H(){ printf '%d\n' "$(echo "$1" | grep -o '"result":"0x[0-9a-fA-F]*"' | cut -d'"' -f4)"; }
```

1. Версия и сеть: `R '{"jsonrpc":"2.0","id":1,"method":"web3_clientVersion","params":[]}'`; `R '{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}'` → `0x38`.
2. Синхронизация: `R '{"jsonrpc":"2.0","id":1,"method":"eth_syncing","params":[]}'` → должно быть `false`. Объект — узел догоняет (смотреть `currentBlock`/`highestBlock`, `txIndexRemainingBlocks`). **Торговлю не включать, пока не `false`.**
3. Отставание от сети:
   ```bash
   OWN=$(H "$(R '{"jsonrpc":"2.0","id":1,"method":"eth_blockNumber","params":[]}')")
   PUB=$(H "$(curl -s -m 10 -X POST -H 'Content-Type: application/json' --data '{"jsonrpc":"2.0","id":1,"method":"eth_blockNumber","params":[]}' https://bsc-dataseed.bnbchain.org)")
   echo "свой $OWN, публичный $PUB, отставание $((PUB-OWN)) блоков"
   ```
   Норма — 0–2 (публичный за балансировщиком сам может отставать на 1–3).
4. Возраст головы: `TS=$(H "$(R '{"jsonrpc":"2.0","id":1,"method":"eth_getBlockByNumber","params":["latest",false]}' | grep -o '"timestamp":"0x[0-9a-f]*"' | sed 's/timestamp/result/')"); echo "$(( $(date +%s) - TS )) с"` → 0–2 с. Заодно `timedatectl` — часы синхронизированы (NTP).
5. Пиры: `R '{"jsonrpc":"2.0","id":1,"method":"net_peerCount","params":[]}'` (десятки — хорошо).
6. Симуляция (как «Проверить подключение»): `R '{"jsonrpc":"2.0","id":1,"method":"eth_simulateV1","params":[{"blockStateCalls":[{"calls":[]}],"validation":false},"latest"]}'` → массив с одним блоком.
7. Multicall3 и eth_call: `R '{"jsonrpc":"2.0","id":1,"method":"eth_getCode","params":["0xcA11bde05977b3631167028862bE2a173976CA11","latest"]}'` (непустой код); `R '{"jsonrpc":"2.0","id":1,"method":"eth_call","params":[{"to":"0xcA11bde05977b3631167028862bE2a173976CA11","input":"0x42cbb15c"},"latest"]}'` (getBlockNumber() — номер блока).
8. Квитанции блока: `R '{"jsonrpc":"2.0","id":1,"method":"eth_getBlockReceipts","params":["latest"]}' | wc -c` (сотни КБ).
9. Газ: `R '{"jsonrpc":"2.0","id":1,"method":"eth_gasPrice","params":[]}'`; `R '{"jsonrpc":"2.0","id":1,"method":"eth_estimateGas","params":[{"from":"0x0000000000000000000000000000000000000001","to":"0x0000000000000000000000000000000000000002","value":"0x0"}]}'` → `0x5208`.
10. Пачка: `R '[{"jsonrpc":"2.0","id":0,"method":"eth_blockNumber","params":[]},{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}]'` → массив из двух.
11. Старое состояние (текст ошибки для П12): `N=$(( OWN - 300 )); R "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"eth_getBalance\",\"params\":[\"0x55d398326f99059fF775485246999027B3197955\",\"$(printf '0x%x' $N)\"]}"` — ожидается ошибка «state 0x… is not available» (записать точный текст).
12. Пределы eth_getLogs: `F=$(printf '0x%x' $((OWN-5001))); T=$(printf '0x%x' $OWN); R "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"eth_getLogs\",\"params\":[{\"address\":\"0x55d398326f99059fF775485246999027B3197955\",\"fromBlock\":\"$F\",\"toBlock\":\"$T\"}]}" | head -c 300` → «exceed maximum block range 5000»; `R '{"jsonrpc":"2.0","id":1,"method":"eth_getLogs","params":[{"address":"0x55d398326f99059fF775485246999027B3197955","fromBlock":"0x1","toBlock":"0x10"}]}'` → ошибка «header not found» / «failed to get logs».
13. Индекс транзакций: `R '{"jsonrpc":"2.0","id":1,"method":"eth_getTransactionReceipt","params":["0x0000000000000000000000000000000000000000000000000000000000000001"]}'` → `null` (индекс готов) или «transaction indexing is in progress».
14. WebSocket: newHeads и предел 1000 вариантов topics (Node.js 26 есть на сервере, `WebSocket` встроен):
    ```bash
    node -e '
    const ws=new WebSocket("ws://127.0.0.1:8546");
    const ids=n=>Array.from({length:n},(_,i)=>"0x"+(i+1).toString(16).padStart(64,"0"));
    const usdt="0x55d398326f99059fF775485246999027B3197955";
    ws.onopen=()=>{
      ws.send(JSON.stringify({jsonrpc:"2.0",id:1,method:"eth_subscribe",params:["newHeads"]}));
      ws.send(JSON.stringify({jsonrpc:"2.0",id:2,method:"eth_subscribe",params:["logs",{address:usdt,topics:[null,ids(1000)]}]}));
      ws.send(JSON.stringify({jsonrpc:"2.0",id:3,method:"eth_subscribe",params:["logs",{address:usdt,topics:[null,ids(1001)]}]}));
    };
    let heads=0; ws.onmessage=e=>{const m=JSON.parse(e.data); if(m.id) console.log("id",m.id,JSON.stringify(m.result??m.error)); else if(++heads<=3) console.log("head",m.params.result.number);};
    setTimeout(()=>process.exit(0),5000);'
    ```
    Ожидание: id 1 и 2 — номера подписок, id 3 — ошибка «exceed max topics», несколько `head`.
15. Окружение бота (П13): `cat /proc/$(pgrep -f '/arb( |$)' | head -1)/environ | tr '\0' '\n' | grep -i proxy` — пусто или `NO_PROXY` с 127.0.0.1.
16. config.toml узла: `grep -nE 'LogQueryLimit|RangeLimit|HTTPModules|WSModules|HTTPHost|WSHost|HTTPVirtualHosts' <путь к config.toml>`.
17. После П1 — в интерфейсе «Проверить подключение» у DEX: все части «Подключено», «История состояния: не нужна».

---

## 8. Что нужно от Тиграна (решения)

1. П1: разрешить `http`/`ws` для loopback-адресов в типе `Url` (совет) или ставить TLS-прокси.
2. П2 + П9: проверка синхронизации и возраста головы (новые настройки, например `bsc.head_max_age_ms`; по желанию — сравнение
   с внешним узлом) — в работе и в «Проверить подключение».
3. П3: `dex.pools_per_sub = 1000` и строка в решении 79 / подсказке настройки про предел geth.
4. П5: резервный узел на время приёмки — пусто или publicnode.
5. П10, П12: по желанию.
