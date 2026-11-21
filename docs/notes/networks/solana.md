# Сеть Solana для бота ARB — разбор (10.11.2026)

> Задача Тиграна 10.11: бот запускается «одна сеть — один сервер — свои кошельки», с открытым или платным узлом
> (не своим). Здесь — что такое Solana для нашего арбитража и что из нашего кода подходит, а что нет.
>
> Правила записки: у каждого факта — ссылка на источник. **«не найдено»** — искал, в первоисточнике нет.
> **[вывод]** — моё рассуждение из фактов, не факт документации; проверить до работы.
> **[сторонний источник]** — не официальная документация протокола, а провайдер / СМИ / агрегатор данных.
> Код бота не менялся и не собирался. Разбор кода «где зашита BSC» по строкам — в соседней записке
> `docs/notes/networks/code-multinet.md`; здесь — только то, что важно именно для Solana.
> Цена SOL для перевода в доллары: **$84,92** — страница MEXC, 2026-11-10 20:55 (UTC+8)
> ([mexc.com/price/SOL](https://www.mexc.com/price/SOL)).

---

## 0. Главное коротко

1. **Solana — не EVM.** Другие адреса (32 байта, base58), другие транзакции (список инструкций, подпись ed25519),
   нет nonce у кошелька, нет мемпула, нет eth_call/eth_simulateV1. Пулы — не контракты с состоянием «внутри», а
   **аккаунты данных**, которые читаются напрямую с узла и на которые можно подписаться.
2. **Ядро, шлюз, биржи, интерфейс — логика переиспользуется, но тип «монета = 20-байтовый адрес EVM» зашит во всём
   проекте** (`core/types/market.rs:29` `bsc_contract: Option<alloy Address>`). Для Solana нужна сквозная смена типа
   адреса монеты — решение Тиграна (подробно — раздел 8).
3. **Адаптер DEX — целиком новый.** Из `venues/bsc_dex` (428 файлов, ~56 тыс. строк) напрямую не переносится ничего,
   кроме **идей**: математика CLMM у Raydium и Orca — это Uniswap V3 (у нас есть V3), DLMM у Meteora — бины
   (у нас есть Infinity Bin), CPMM/AMM v4/PumpSwap — x·y=k (у нас есть V2). Но числа в другом формате (Q64.64 вместо
   Q96), другие комиссии, другое хранение тиков.
4. **Почти половина объёма Solana — «закрытые» prop AMM** (BisonFi, HumidiFi, GoonFi, SolFi, Tessera V, ZeroFi):
   их логика не опубликована, доступ — через агрегатор Jupiter. Своими пулами их не посчитать.
5. **Jupiter** — сильный кандидат для ноги «биржа ↔ DEX» (`/build` без комиссии Jupiter, отдаёт инструкции,
   до 150 запросов/с на тарифе $500/мес). Для DEX ↔ DEX (цикл в ту же монету) обычный Jupiter API **не годится** —
   круговые маршруты не поддерживаются.
6. **Атомарность в Solana встроена**: все инструкции одной транзакции — всё-или-ничего; бандл Jito — до 5 транзакций,
   всё-или-ничего, в одном слоте. Свой контракт-исполнитель (как ArbExecutor) для простого цикла не обязателен [вывод].
7. **Стоимость свопа — доли цента** (база 5 000 лампортов + приоритет + чаевые), но **чаевые за скорость** (Jito /
   Helius Sender) — 0,001 SOL ≈ $0,085 — и это главная затрата исполнения.

---

## 1. Устройство сети для арбитража

### 1.1 Слоты, лидеры, финальность

| Факт | Значение | Источник |
| --- | --- | --- |
| Время слота в коде клиента (константа) | 400 мс, `DEFAULT_MS_PER_SLOT = 400` | [solana-clock 3.2.0, lib.rs](https://docs.rs/crate/solana-clock/3.2.0/source/src/lib.rs) |
| Время слота сейчас | **200 мс** — снижение с 400 мс завершено в октябре 2026 [сторонний источник] | [Chainstack docs: Alpenglow](https://docs.chainstack.com/docs/solana-alpenglow-consensus), [RPC Fast, 27.10.2026](https://rpcfast.com/blog/alpenglow-solana-150ms-finality-explained) |
| Лидер подряд | 4 слота, `NUM_CONSECUTIVE_LEADER_SLOTS = 4` (с 3.1.0 перенесено в крейт solana-leader-schedule) | [solana-clock](https://docs.rs/crate/solana-clock/3.2.0/source/src/lib.rs) |
| Эпоха | 432 000 слотов | там же |
| Срок жизни blockhash транзакции | 150 (`MAX_PROCESSING_AGE`); на странице транзакций — «150 slots» | там же; [solana.com/docs/core/transactions](https://solana.com/docs/core/transactions) |
| Новый консенсус Alpenglow (Votor) | включён в mainnet 21.10.2026, слот 459 653 000; цель финальности ~150 мс; Rotor (замена Turbine) ещё не включён [сторонний источник] | [Chainstack](https://docs.chainstack.com/docs/solana-alpenglow-consensus), [RPC Fast](https://rpcfast.com/blog/alpenglow-solana-150ms-finality-explained). Официальный анонс Anza — **не прочитан** (страницы СМИ не открылись) |

### 1.2 Подтверждения (commitment)

Официальная документация RPC ([solana.com/docs/rpc](https://solana.com/docs/rpc)):
- `processed` — самый свежий блок узла, «может быть откатан»;
- `confirmed` — блок, за который проголосовали больше 2/3 стейка;
- `finalized` — финализирован; **по умолчанию** методы берут `finalized`.

После Alpenglow [сторонний источник, Chainstack]: `confirmed` и `finalized` — одно и то же состояние, `confirmed`
планируют убрать; в `getSignatureStatuses` поле `confirmations` = 0 до финальности, смотреть `confirmationStatus`;
в одном слоте может быть несколько кандидатов-блоков (`bank_id`); голосующие транзакции исчезли из потоков Geyser.

**Для бота [вывод]:** цены слушать на `processed` (как «голова» на BSC), сделку считать сделанной на `finalized`.

### 1.3 Нет мемпула, кто включает транзакцию

- Документация Jito и solana.com про мемпул **ничего не говорят** (не найдено). Транзакция уходит к текущему/
  ближайшим лидерам (узел RPC пересылает). Публичного мемпула, который можно слушать, как на BSC, нет — **не найдено
  официального описания**, только сторонние статьи (Helius «Gulf Stream»), не читались.
- Порядок в блоке определяют **приоритетная комиссия** и **чаевые Jito** (у лидеров с клиентом Jito-Solana).

### 1.4 Jito: бандлы и чаевые (официальная документация)

Источник: [docs.jito.wtf/lowlatencytxnsend](https://docs.jito.wtf/lowlatencytxnsend/).

| Что | Факт |
| --- | --- |
| Методы Block Engine `/api/v1` | `sendTransaction` (всегда skip_preflight=true), `sendBundle`, `getBundleStatuses`, `getInflightBundleStatuses` (последние 5 минут), `getTipAccounts` |
| Бандл | **до 5 транзакций**, выполняются **по порядку**, **всё-или-ничего**, в **одном слоте** |
| Чаевые | перевод SOL на один из **8 счетов чаевых** (`getTipAccounts`, выбирать случайно); минимум **1 000 лампортов** («может не хватить при высоком спросе») |
| Где класть чаевые | в ту же транзакцию, что и основная логика; отдельная транзакция чаевых — риск «uncle bandit» |
| Таблицы адресов (ALT) | для счетов чаевых **не использовать** |
| На лидерах без Jito | чаевые транзакцию не продвигают |
| `sendTransaction` | совет делить 70/30 приоритетная комиссия / чаевые; `bundleOnly=true` — отправка как бандл из одной транзакции (защита от отката) |
| Уровень чаевых | REST `GET https://bundles.jito.wtf/api/v1/bundles/tip_floor` (перцентили 25–99 и EMA 50-го), WS `wss://bundles.jito.wtf/api/v1/bundles/tip_stream` |
| Лимит | **1 запрос/с на IP на регион** без ключа; выше — заявка в Discord; ключ — заголовок `x-jito-auth` |
| Регионы mainnet | Amsterdam, Dublin, Frankfurt, London, New York, Salt Lake City, Singapore, Tokyo + global |
| Кодировка | base64 (base58 — «медленно, устарело») |
| Риски | «uncled blocks»: при пропуске слота транзакции бандла могут попасть в сеть **без атомарности**; совет — проверки до/после внутри транзакции |
| `simulateBundle` | не метод Block Engine; «должен поддерживаться» RPC Jito-Solana |

Живое значение tip_floor в этой работе **получить не удалось** (запрос не разрешён в сессии) — бот должен брать его
сам из `tip_floor`/`tip_stream`, без зашитых чисел.

Дополнительно — **Helius Sender** (канал отправки, аналог наших «каналов» на BSC) ([helius.dev/docs/sending-transactions/sender](https://www.helius.dev/docs/sending-transactions/sender)):
шлёт сразу по нескольким путям (Helius, Jito, Harmonic, Rakurai); регионы slc, ewr, lon, **fra**, **ams**, sg, tyo;
минимум чаевых **0,001 SOL** (Sender Max, все пути) или 0,000005 SOL (один путь SWQOS); без ключа 1 запрос/с на IP
на регион, с ключом 50 запросов/с; кредиты не тратит, есть на бесплатном тарифе.

### 1.5 Атомарность и размеры транзакции

| Факт | Значение | Источник |
| --- | --- | --- |
| Все инструкции одной транзакции | «All instructions succeed or all revert. Fees are still charged on failure» | [solana.com/docs/core/transactions](https://solana.com/docs/core/transactions) |
| Размер транзакции | 1 232 байта (legacy, v0); **4 096 байт** у формата **v1** | там же; [versioned-transactions](https://solana.com/docs/core/transactions/versioned-transactions#v1-format) |
| Аккаунтов на транзакцию | 64 (v0 — через таблицы адресов); v1 — 64 прямо в транзакции (96 с SIMD-0596) | там же |
| Таблицы адресов (ALT) | есть в v0; в **v1 убраны намеренно** | [versioned-transactions](https://solana.com/docs/core/transactions/versioned-transactions#v1-format) |
| Формат v1 | «active on mainnet, devnet, and testnet»; лимиты вычислений и данных по умолчанию **0** — задавать обязательно | там же |
| Исполняемых инструкций | 64 (верхние + вложенные CPI) | [transactions](https://solana.com/docs/core/transactions) |
| Подпись | 64 байта, ed25519 | там же |
| Вычисления (CU) на транзакцию | максимум **1 400 000**; по умолчанию 200 000 на инструкцию (3 000 у встроенных) | [compute-budget](https://solana.com/docs/core/fees/compute-budget) |
| CU на блок / на один записываемый аккаунт за блок | 60 000 000 / 12 000 000 | там же |
| Данные загружаемых аккаунтов | максимум 64 МиБ | там же |

**Для бота [вывод]:** цикл DEX ↔ DEX из 2–3 свопов помещается в одну транзакцию (своп Raydium ≈ 140–320 тыс. CU,
раздел 2) — атомарность «из коробки», без своего контракта-исполнителя. Предел — аккаунты: своп CLMM тянет пул,
хранилища, несколько tick array, оракул — 3 свопа легко упираются в 64 аккаунта, поэтому v0 + ALT или v1.

### 1.6 Где ставить сервер

Факты (где у сервисов точки присутствия в Европе):
- Jito Block Engine: Amsterdam, Dublin, **Frankfurt**, London ([Jito](https://docs.jito.wtf/lowlatencytxnsend/));
- Helius Sender: London, **Frankfurt**, **Amsterdam** ([Helius](https://www.helius.dev/docs/sending-transactions/sender));
- шлюз Jupiter API в Европе — **Frankfurt** (AWS eu-central-1) ([Jupiter latency](https://developers.jup.ag/docs/portal/latency.md));
- Triton Shred Streaming — Frankfurt и Amsterdam ($1 500/мес) дороже, чем London/New York/Tokyo ($900/мес)
  ([triton.one/pricing](https://triton.one/pricing)).
- Доля стейка/лидерских слотов по городам — официального источника **не найдено** (заголовки СМИ о концентрации в
  Европе видел, страницы не открылись).

**[вывод]** Сервер — Франкфурт (или Амстердам): там сходятся Jito, Sender, Jupiter. Текущий сервер BSC (OVH,
Гравлин, Франция — из памяти проекта) ближе к Амстердаму/Франкфурту, но для Solana нужен отдельный сервер по правилу
«одна сеть — один сервер».

---

## 2. Комиссии

| Что | Факт | Источник |
| --- | --- | --- |
| Базовая | **5 000 лампортов за подпись**; 50% сжигается, 50% валидатору; берётся **и при неудаче** | [fee-structure](https://solana.com/docs/core/fees/fee-structure) |
| Приоритетная | `ceil(цена_CU (микролампорты) × лимит_CU / 1 000 000)` лампортов; 100% валидатору; считается от **запрошенного** лимита CU, не от израсходованного | там же |
| Приоритетная в v1 | абсолютная сумма в лампортах, без умножения | там же |
| Чаевые Jito | минимум 1 000 лампортов; живой уровень — tip_floor | [Jito](https://docs.jito.wtf/lowlatencytxnsend/) |
| Чаевые Helius Sender | минимум 0,001 SOL (все пути) / 0,000005 SOL (один путь) | [Helius Sender](https://www.helius.dev/docs/sending-transactions/sender) |
| 1 SOL | 1 000 000 000 лампортов | [fees](https://solana.com/docs/core/fees.md) (микролампортов в лампорте — 1 000 000) |

Расход вычислений свопа (официальная документация Raydium, ±15%,
[priority-fee-tuning](https://docs.raydium.io/integration-guides/priority-fee-tuning.md)):
AMM v4 — 140 000 CU; CPMM — 140 000–150 000 (Token-2022 — 180 000–200 000); CLMM — 170 000 (без пересечения тика),
220 000 (1 тик), 320 000 (3 тика). Совет — лимит CU на ~25% выше ожидаемого. Пример бота в той же документации:
~20 000 микролампортов/CU (50-й перцентиль) × 250 000 CU ≈ 5 000 лампортов на транзакцию. Признак перегрузки:
75-й перцентиль приоритета > 500 000 микролампортов/CU или 50-й перцентиль чаевых Jito > 0,001 SOL.

**Стоимость одного свопа [вывод — мой расчёт по фактам выше, SOL = $84,92 на 10.11.2026]:**

| Сценарий | Лампорты | SOL | USD |
| --- | --- | --- | --- |
| База 5 000 + приоритет 5 000 (пример Raydium) | 10 000 | 0,00001 | ≈ $0,00085 |
| + минимальные чаевые Jito 1 000 | 11 000 | 0,000011 | ≈ $0,00093 |
| База + приоритет при перегрузке (500 000 мкл/CU × 250 000 CU = 125 000) | 130 000 | 0,00013 | ≈ $0,011 |
| + чаевые 0,001 SOL (Sender Max / «перегрузка» Jito) | ~1 130 000 | ~0,00113 | ≈ $0,096 |

Важно для бота: **неудачная транзакция тоже платит базу и приоритет** (в бандле Jito — не попадает в блок, значит не
платит [вывод из «only successful transactions can be included»]).

---

## 3. Узлы (RPC): публичные и платные

### 3.1 Публичный узел

`https://api.mainnet.solana.com` — «shared infrastructure, not intended for production»; при превышении — `429`,
при блокировке — `403`; числа лимитов на странице **не указаны** ([solana.com/docs/rpc](https://solana.com/docs/rpc)).
**[вывод]** Для бота не годится.

### 3.2 Платные провайдеры (официальные страницы цен)

| Провайдер | Тарифы / цена | Запросов/с | Потоки (gRPC Geyser) | Источник |
| --- | --- | --- | --- | --- |
| **Helius** | Free $0 (1 млн кредитов), Developer $49 (10 млн), Business $499 (100 млн), Professional $999 (200 млн) | 10 / 50 / 200 / 500; sendTransaction 1 / 5 / 50 / 100; getProgramAccounts 5 / 25 / 50 / 75 | LaserStream gRPC: mainnet — с Business; Developer — только devnet; доп. трафик (Professional) 5 ТБ $400 … 100 ТБ $6 000 | [helius.dev/pricing](https://www.helius.dev/pricing.md) |
| **Triton** | Pay-as-you-go: депозит от $125; $10 за млн вызовов + $0,08/ГБ; потоки — $0,08/ГБ | «гибкие», числа на странице нет; по Corvus — ~120 запросов/с в среднем [сторонний источник] | Dragon's Mouth (Yellowstone — их разработка); отдельной цены на странице нет | [triton.one/pricing](https://triton.one/pricing), [Corvus, 25–26.07.2026](https://docs.corvus-labs.io/platform/pricing-comparison) |
| **QuickNode** | Accelerate $249, Scale $499 (950 млн кредитов), Business $999 (2 млрд) | Scale 250, Business 500 | gRPC входит в Scale (10 потоков × 10 аккаунтов) и Business (25 × 50) с 01.06.2026; иначе доп. $499; 10 кредитов за 0,1 МБ | [QuickNode blog](https://www.quicknode.com/blog/solana-grpc-is-now-included-with-scale-and-business-plans) |
| **Alchemy** | gRPC: pay-as-you-go от **$75 за ТБ**; на бесплатном тарифе нет | не указано | Yellowstone gRPC | [support.alchemy.com](https://support.alchemy.com/articles/5952597732-how-is-solana-grpc-yellowstone-streaming-priced) |
| **Chainstack** | Yellowstone add-on $49/мес (2 потока), $149/мес (7 потоков), на тарифе Growth и выше | Developer (бесплатно) — 5 запросов/с для Solana [сторонний источник RPC Fast] | Yellowstone gRPC | [chainstack.com, 11.06/06.07.2026](https://chainstack.com/yellowstone-grpc-more-streams-same-price/), [RPC Fast](https://rpcfast.com/blog/solana-rpc-pricing) |

Задержка между провайдерами: по замеру Corvus (сам провайдер, [сторонний источник]) Helius LaserStream и Triton
Dragon's Mouth отстают от Corvus на ~2 мс по медиане. Независимых официальных замеров **не найдено**.

### 3.3 Что даёт узел для нашего бота

**WebSocket** ([Helius, оптимизация](https://helius.dev/docs/rpc/optimization-techniques)):
- `accountSubscribe` — один аккаунт (пул), можно `dataSlice` — **аналог нашей подписки на событие пула**, только
  приходит само **новое состояние** пула, а не событие Swap для пересчёта;
- `programSubscribe` — все аккаунты программы (DEX) с фильтрами `dataSize`, `memcmp`;
- `logsSubscribe` — логи транзакций программы.

**Yellowstone gRPC (Geyser)** ([описание](https://www.mintlify.com/rpcpool/yellowstone-grpc/architecture)):
плагин внутри валидатора; потоки аккаунтов, транзакций, слотов, блоков, entries; фильтры аккаунтов по pubkey,
владельцу (программе), размеру, memcmp, лампортам; `from_slot` — догон пропущенного (если узел хранит слоты);
унарные `GetLatestBlockhash`, `GetSlot` и др. Rust-клиент — крейт `yellowstone-grpc-client`
([docs.rs](https://docs.rs/yellowstone-grpc-client)). **[вывод]** Для «каждая мс важна» gRPC быстрее и надёжнее WS;
это то, ради чего стоит платный тариф.

**`simulateTransaction`** ([solana.com](https://solana.com/docs/rpc/http/simulatetransaction)):
прогон подписанной транзакции без отправки: `sigVerify`, `replaceRecentBlockhash`, `innerInstructions`, `accounts`;
в ответе `err`, `logs`, `unitsConsumed`, `returnData`, pre/post балансы токенов. Это **аналог нашего
eth_simulateV1** для проб «налогов» и сухой сделки. **Подмены состояния аккаунтов (state override, как в
eth_simulateV1) в документации не найдено** — значит пробы «купить и продать» с подставленным балансом, как на BSC,
так не сделать [вывод]; пробу можно делать только реальным кошельком с реальным балансом.

**`getProgramAccounts`** — выгрузка всех аккаунтов программы с фильтрами `dataSize` + `memcmp` (например, по mint на
нужном смещении), `dataSlice`; без пагинации «может упасть по таймауту на больших наборах»; у Helius есть
`getProgramAccountsV2` с `changedSinceSlot` и пагинацией ([Helius](https://helius.dev/docs/rpc/optimization-techniques)).
Лимит у Helius — 5–75 запросов/с по тарифу.

**Свой справочник пулов [вывод]:** аналог решения 58 (индексатор по REST) — либо `getProgramAccounts` по каждой
программе DEX с фильтром mint (редко, при старте и по интервалу), либо REST-API самих DEX (у Raydium/Meteora/Orca
свои API — **в этой работе не проверены**). Живые изменения — `accountSubscribe`/gRPC по найденным пулам.

---

## 4. DEX сети Solana

### 4.1 Доли объёма

**DefiLlama** (страница [defillama.com/protocols/Dexs/Solana](https://defillama.com/protocols/Dexs/Solana), снято
10.11.2026; даты на странице нет; итог Solana: 24ч $1,58 млрд, 30 дней $46,18 млрд; доля — моё деление):

| # | Протокол | 30 дней | Доля 30д | Вид [источник вида] |
| --- | --- | --- | --- | --- |
| 1 | BisonFi | $7,37 млрд | 16,0% | prop AMM ([SolanaFloor, 19.05.2026](https://solanafloor.com/news/solana-prop-amm-volume-share-drops-below-50-first-time-since-november-2025)) |
| 2 | Orca | $6,51 млрд | 14,1% | Whirlpools (CLMM) |
| 3 | Manifest Trade | $5,05 млрд | 10,9% | стакан (CLOB), крейт `manifest-dex` ([docs.rs](https://docs.rs/crate/manifest-dex/2.0.0)) |
| 4 | Meteora | $4,59 млрд | 9,9% | DLMM, DAMM v1/v2, DBC |
| 5 | Raydium | $4,17 млрд | 9,0% | AMM v4, CPMM, CLMM |
| 6 | GoonFi | $3,46 млрд | 7,5% | prop AMM ([Helius](https://www.helius.dev/blog/solanas-proprietary-amm-revolution)) |
| 7 | Scorch | $2,73 млрд | 5,9% | вид **не установлен** |
| 8 | SolFi | $2,22 млрд | 4,8% | prop AMM (Ellipsis Labs) ([Helius](https://www.helius.dev/blog/solanas-proprietary-amm-revolution)) |
| 9 | HumidiFi | $1,62 млрд | 3,5% | prop AMM (там же) |
| 10 | Pump | $1,47 млрд | 3,2% | PumpSwap (x·y=k) + кривая pump.fun |
| 11 | AlphaQ | $1,38 млрд | 3,0% | вид **не установлен** |
| 12 | Aquifer | $1,30 млрд | 2,8% | вид **не установлен** |
| 13 | Tessera V | $0,98 млрд | 2,1% | prop AMM (Wintermute) ([Helius](https://www.helius.dev/blog/solanas-proprietary-amm-revolution)) |
| 14 | Jupiter | $0,95 млрд | 2,1% | — |
| 15 | PancakeSwap | $0,90 млрд | 1,9% | V3 на Solana |
| … | ZeroFi 1,2%, Byreal 0,8%, Whalestreet 0,6%, Archer, MetaDao, FluxBeam, DefiTuna, Saber | | | ZeroFi — prop AMM (Helius) |

Другие срезы (для сравнения, методики разные): DexPaprika 10.11.2026 19:42 UTC, 24ч — PumpSwap $1,11 млрд, Meteora
DAMM V2 $306 млн, Orca $217 млн, Raydium CLMM $117 млн, Meteora $116 млн ([dexpaprika.com](https://dexpaprika.com/solana/dexes));
Bitquery 23.04.2026 — Meteora ~58% ([docs.bitquery.io](https://docs.bitquery.io/docs/mcp/trading/examples/solana-dex-market-share/)).
**Цифры у агрегаторов сильно расходятся** (prop AMM DexPaprika вообще не показывает) — до решения о приоритетах
сравнить по своим данным узла.

**Prop AMM — главное отличие Solana** ([Helius, 25.08.2025](https://www.helius.dev/blog/solanas-proprietary-amm-revolution)):
маркетмейкер своим ключом «несколько раз в секунду» обновляет кривую; «их логика обмена не раскрыта», обычно нет
публичных IDL, документации; доступ — через роутер Jupiter (интеграция по разрешению, SDK передан Jupiter, не
опубликован). Если оракул перестаёт обновляться — свопы в пуле останавливаются. Доля prop AMM в недельном объёме —
47% (Blockworks, по [SolanaFloor 19.05.2026](https://solanafloor.com/news/solana-prop-amm-volume-share-drops-below-50-first-time-since-november-2025)).
**[вывод]** Своей математикой их не посчитать — только котировка Jupiter.

### 4.2 Математика DEX и сравнение с нашей (`venues/bsc_dex/math`)

| DEX / вид | Математика (факт) | Похоже на наше | Как читать состояние |
| --- | --- | --- | --- |
| **Raydium AMM v4** `675kPX9M…1Mp8` | x·y=k; связка с OpenBook **отключена** — свопы только по кривой; ~0,25% комиссия; **Token-2022 не поддерживает**; новые пулы UI/SDK создаёт в CPMM ([versions](https://docs.raydium.io/protocol-overview/versions-and-migration), [адреса](https://docs.raydium.io/reference/program-addresses)) | V2 | аккаунт пула + балансы двух хранилищ (vault) |
| **Raydium CPMM** `CPMMoo8L…qKP1C` | «pure x·y=k, no orderbook, native Token-2022» | V2 (+ учёт налога Token-2022) | аккаунт пула + хранилища |
| **Raydium CLMM** `CAMMCzo5…rKgrWqK` | sqrt_price **Q64.64**; тик 1.0001; шаг свопа «совпадает с Uniswap v3 `computeSwapStep` один в один»; комиссия в миллионных; `SwapV2` для Token-2022 с налогом ([math](https://docs.raydium.io/products/clmm/math.md)); загружает до 8 tick array ([priority-fee-tuning](https://docs.raydium.io/integration-guides/priority-fee-tuning.md)) | **V3** (только Q64 вместо Q96) | пул + аккаунты tick array (+ битмап) |
| **Orca Whirlpools** | тик 1.0001, диапазон тиков ±443636, sqrt-цена в [2⁻⁶⁴, 2⁶⁴] ([price-and-ticks](https://docs.orca.so/developers/architecture/price-and-ticks)); tick array = 88 тиков (~10 КБ), PDA от пула и стартового тика; своп смотрит **до 3** tick array, дальше — ошибка; неинициализированные массивы проходятся ([tick-arrays](https://docs.orca.so/developers/architecture/tick-arrays)) | **V3** | пул + 3 tick array вокруг цены; официальный Rust-крейт `orca_whirlpools_core` — математика и котировки ([SDK](https://docs.orca.so/developers/sdks/overview)) |
| **Meteora DLMM** | цена бина = (1 + bin_step/10 000)^id, Q64.64; внутри бина **постоянная сумма** L = цена·x + y; комиссия = базовая + переменная (от «накопителя волатильности»), предел MAX_FEE_RATE ([formulas](https://docs.meteora.ag/overview/products/dlmm/dlmm-formulas)) | **Infinity Bin / Liquidity Book** (та же идея бинов), но **своя динамическая комиссия** | пул (lb_pair) + bin array; официальный Rust: крейт `commons` в репозитории `dlmm-sdk` — `quote_exact_in` / `quote_exact_out`, `get_bin_array_pubkeys_for_swap` ([rust-integration](https://docs.meteora.ag/developer-guides/dlmm/rust-integration/library)); размер bin array — **не найдено** |
| **Meteora DAMM v2** | x·y=k (полный диапазон) **или** концентрированный в диапазоне √P_min…√P_max; цена хранится как √P; комиссия ⌈сумма × числитель / 10⁹⌉ + необязательная динамическая ([formulas](https://docs.meteora.ag/core-products/damm-v2/formulas), [dynamic-fees](https://docs.meteora.ag/core-products/damm-v2/fees/dynamic-fees)); есть «расписание» базовой комиссии для пулов запуска; формулы свопа и Token-2022 — **не найдено** на этих страницах | V2 / V3 с одним диапазоном | аккаунт пула |
| **PumpSwap** `pAMMBay6…FMfXEA` | «constant-product AMM for migrated tokens»; три комиссии: LP, протокол (8 получателей), **создатель монеты**; ставки по ступеням капитализации в `FeeConfig` отдельной программы ([pump-public-docs, deepwiki](https://deepwiki.com/pump-fun/pump-public-docs)) — это пересказ публичных docs pump.fun | V2 + комиссия, зависящая от капитализации | Pool, GlobalConfig, FeeConfig |
| **Manifest** | стакан (CLOB), Rust-крейт `manifest-dex` ([docs.rs](https://docs.rs/crate/manifest-dex/2.0.0)) — подробности **не читались** | стакан биржи (не пул) | аккаунт рынка |
| **Phoenix** `PhoeNiXZ…GqdXY` | «Phoenix Legacy is an on-chain orderbook that operates without a crank» ([GitHub](https://github.com/Ellipsis-Labs/phoenix-v1)); в топе DefiLlama **нет** | стакан | аккаунт рынка |
| **OpenBook** | в этой работе **не исследовался** (у Raydium AMM v4 связка отключена) | — | — |
| **Lifinity** | оракульный AMM, свой оракул, x·y = c·k ([docs.lifinity.io](https://docs.lifinity.io/llms-full.txt)); последние даты в документации — 2023; в топе DefiLlama **нет**; жив ли — **не найдено** | — (оракул) | — |
| **Prop AMM** (BisonFi, HumidiFi, GoonFi, SolFi, Tessera V, ZeroFi) | логика закрыта | — | только Jupiter |

**[вывод]** Наш код математики V2/V3/Bin — хорошая основа по смыслу, но переписывается: Q64.64 вместо Q96, u128
вместо U256, свои правила комиссий (динамические у DLMM/DAMM, по капитализации у PumpSwap), свои структуры
tick/bin array. Плюс есть официальные Rust-крейты Orca и Meteora — их можно взять для сверки (как квотер на BSC).

---

## 5. Агрегаторы

### 5.1 Jupiter (официальная документация, [developers.jup.ag](https://developers.jup.ag/docs/swap/index.md))

| Что | Факт |
| --- | --- |
| Два пути | **`/order` + `/execute`** — готовая транзакция, все роутеры (Metis, JupiterZ, Dflow, OKX), есть комиссия Jupiter; **`/build` + `/submit`** — сырые инструкции, только Metis, **«/build has no Jupiter swap fee»**, полный контроль транзакции |
| Комиссия `/order` | от 0 bps (привязанные активы, токены Jupiter) до **10 bps** на большинстве пар; **50 bps** на токенах моложе 24 ч |
| `/build` отдаёт | setup/swap/cleanup/other/tip/computeBudget инструкции + `addressesByLookupTableAddress` (для v0; при `transactionVersion=1` — пусто) + котировку (`inAmount`, `outAmount`, `routePlan`) ([build](https://developers.jup.ag/docs/swap/build/index.md)) |
| Параметры `/build` | `inputMint`, `outputMint`, `amount`, `taker`; `slippageBps` (по умолч. 50 или `rtse`), `dexes`, `excludeDexes`, `maxAccounts` 1–64; `mode=fast` — «reduced latency routing (BETA)» |
| Свои инструкции рядом | да — можно добавлять свои инструкции в ту же транзакцию |
| Круговой маршрут (A → … → A) | «circular arbitrage routes are not supported and require the Metis binary» — фраза из сводного индекса документации ([llms.txt](https://developers.jup.ag/docs/llms.txt)); на самой странице advanced **не подтвердилось** — проверить |
| Свой Metis (бинарник) | ключ только при **10 000 застейканных JUP**, цена не указана ([Metis v7, 17.11.2025](https://developers.jup.ag/blog/metis-v7)) |
| Тарифы ([plans](https://developers.jup.ag/docs/portal/plans)) | Free $0 — 1 запрос/с; Developer $25 — 10/с; Launch $100 — 50/с; **Pro $500 — 150/с**; без ключа 0,5/с |
| Лимиты ([rate-limits](https://developers.jup.ag/docs/portal/rate-limits)) | окно 60 с скользящее; лимит **на организацию**, не на ключ; `/swap/v2/execute` — свой лимит 20–100/с; заголовки `x-ratelimit-*` |
| Задержка ([latency](https://developers.jup.ag/docs/portal/latency.md)) | шлюз в 6 регионах AWS (в Европе — Frankfurt); чисел задержки **нет** |
| Цены / токены | Price API v3, Tokens API v2 ([llms.txt](https://developers.jup.ag/docs/llms.txt)) |

### 5.2 OKX DEX API

Через SDK ([mintlify okx-dex-sdk](https://www.mintlify.com/okx/okx-dex-sdk/guides/solana-swaps)): котировка,
своп, **инструкции для Solana** (`getSolanaSwapInstruction` → `instructionLists`, для сборки своей транзакции);
chainIndex 501; авторизация — ключ, секрет, пароль, project ID. Лимиты запросов и базовый URL — **не найдено** на
этой странице. Официальная страница web3.okx.com в этой работе **не читалась**.

### 5.3 Свои пулы или Jupiter — оценка [вывод]

| Направление | Только свои пулы | Только Jupiter | Смешанно |
| --- | --- | --- | --- |
| **Биржа ↔ DEX** | не видим ~половину объёма (prop AMM); много работы (5+ видов пулов) | видим всё, включая prop AMM; `/build` без комиссии; но каждая котировка — HTTP до Франкфурта, 150 запросов/с на $500 — **не хватит** на постоянный пересчёт всех монет | **свой поток пулов** для сигнала (WS/gRPC, без HTTP) + **Jupiter `/build`** для финальной котировки и сделки — то же, что наш «лучший из двух» с KyberSwap на BSC (решение 123) |
| **DEX ↔ DEX** | единственный путь: круг Jupiter API не строит; prop AMM в цикл не взять | не поддерживается | свои пулы + несколько `/build` по ногам в одну транзакцию — возможно, но котировки разных вызовов не согласованы между собой — **проверить** |

Ответ на вопрос «не проще ли целиком через Jupiter»: **для биржа ↔ DEX — как исполнитель и финальная котировка да,
как источник постоянного потока цен — нет** (лимиты, HTTP-задержка, правило «всё слушаем через WebSocket»).
**Для DEX ↔ DEX — нет** (круговые маршруты не поддерживаются).

---

## 6. Токены: аналог «налогов»

| Риск | Что это (факт) | Как проверять [вывод] |
| --- | --- | --- |
| **Программа токена** | две: классическая Token Program (неизменяемая) и Token-2022 (Token Extensions) ([solana.com/docs/tokens](https://solana.com/docs/tokens)) | владелец аккаунта mint = какая программа |
| **Freeze authority** | необязательный ключ, который может **заморозить** токен-аккаунт: «preventing them from being transferred or burned» — есть и в классической программе ([tokens](https://solana.com/docs/tokens)) | поле `freeze_authority` в mint: задан — риск «заморозят наш кошелёк» |
| **Transfer fee** (Token-2022) | ставка в bps + максимальная сумма; удерживается на счёте получателя; приходит меньше; смена ставки — через **2 эпохи**; `transfer_checked_with_fee` сверяет комиссию ([transfer-fees](https://solana.com/pl/docs/tokens/extensions/transfer-fees)) | **читается из mint напрямую** — это точнее нашей пробы налога на BSC: ставка известна заранее и на 2 эпохи вперёд |
| **Transfer hook** (Token-2022) | каждый перевод вызывает чужую программу; «может отклонить перевод по любой причине»; программу хука автор может сменить (`UpdateTransferHook`), список доп. аккаунтов тоже — «перечитывать для каждого перевода» ([transfer-hook-integration](https://solana.com/pl/docs/tokens/extensions/transfer-hook-integration)) | аналог «крючков» V4 на BSC: только симуляция `simulateTransaction` на каждую сделку; по умолчанию — не торговать (настройка) |
| **Permanent delegate** | может переводить и **сжигать** токены с **любого** аккаунта этого mint ([permanent-delegate](https://solana.com/pl/docs/tokens/extensions/permanent-delegate)) | расширение в mint: есть — не торговать (настройка) |
| **Default account state** | новые аккаунты могут создаваться **замороженными** до разморозки freeze authority ([default-state](https://solana.com/pl/docs/tokens/extensions/default-state)) | расширение в mint |
| **Pausable, NonTransferable** | пауза переводов / непереводимый токен ([extensions](https://solana.com/pl/docs/tokens/extensions)) | расширения в mint |

**[вывод]** На Solana «налог» большей частью виден **из данных mint без пробы** (transfer fee, delegate, pause,
freeze). Проба симуляцией нужна только для transfer hook и для prop AMM / хитрых пулов.

---

## 7. Биржи и Solana

### 7.1 Факты из записанных ответов API (в репозитории)

| Биржа | Источник | Что есть по Solana |
| --- | --- | --- |
| **BingX** | `tests/data/bingx_2026-10-24/config_getall.json` (20 монет, живой ответ 24.10.2026) | USDT, сеть `SOL`: ввод и вывод открыты, комиссия вывода **1 USDT**, минимум 1, `minConfirm` **50**, `contractAddress` = `Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB` (mint USDT в Solana) |
| **Gate** | `tests/data/cex_samples_2026_11_04/gate_spot_currencies.json` (04.11.2026) | 5 699 валют; сеть `SOL` у **383**, из них торгуются 185, ввод+вывод SOL открыты у **171**; адрес = mint (base58, 43–44 символа): USDT `Es9vMF…`, USDC `EPjFWdd5…`, JUP `JUPyiw…`, BONK `DezXAZ…`, WIF `EKpQGS…`, RAY `4k3Dyj…`; у SOL адрес пустой. Комиссий вывода в этом ответе нет |
| **KuCoin** | `tests/data/cex_samples_2026_11_04/kucoin_currencies_v3.json` (04.11.2026) | 2 273 валюты; `chainId = sol` у **200**, ввод+вывод открыты у **95**; `confirms` **200**; вывод: USDT 1,5 (мин. 3), USDC 1 (мин. 5), SOL 0,008, JUP 3, BONK 240 000; `contractAddress` = mint |

### 7.2 Остальные биржи

Binance: комиссии «меняются без уведомления», список — страница [binance.com/en/fee/cryptoFee](https://www.binance.com/en/fee/cryptoFee)
([FAQ](https://www.binance.com/en/support/faq/d9ff84dccc24423cbe6d59859d86afe3)). MEXC: «Withdrawal fees are
determined by the respective blockchain network», смотреть на странице вывода ([MEXC](https://www.mexc.com/support/articles/17827791528721));
у MEXC есть объявление «Suspension of Withdrawals on the Solana (SOL) Network» — видел только заголовок в поиске,
страница **не открылась**, дата и причина **не найдены**.

Bybit, Bitget, OKX, XT, HTX, CoinEx, Kraken, Phemex, Poloniex — по Solana в этой работе **не проверено**. Источник
фактов уже есть в боте: тот же справочник сетей, что адаптеры читают для BSC (`coin_networks` каждой биржи), — в нём
сеть Solana приходит рядом с BSC. **[вывод]** Самый надёжный путь — один живой прогон справочника каждой биржи с
фильтром по имени сети Solana (у каждой биржи своё имя: у BingX и Gate — `SOL`, у KuCoin chainId `sol`), как это
делалось для `venue.bsc_network`.

### 7.3 Совпадение адресов

- У BingX, Gate, KuCoin адрес монеты в сети Solana — **это mint-адрес** (base58), тот же, что в пулах DEX: USDT
  `Es9vMF…`, USDC `EPjFWdd5…` (по записанным ответам). Значит правило решения 211 «монета = адрес» на Solana работает.
- **Но сейчас бот этот адрес потеряет:** BingX разбирает `contractAddress` как EVM-адрес — `.parse().ok()`
  (`venues/bingx/wallet.rs:77`), base58 не разберётся → `None` → монета «без адреса». Так во всех адаптерах бирж
  (поле `bsc_contract: Option<Address>`, `core/types/market.rs:29`).
- Время зачисления: биржи отдают число подтверждений (BingX 50, KuCoin 200), в чём они считаются (слоты, блоки) —
  **в документации не найдено**. Реальное время — только замер на живом переводе.

---

## 8. Вывод: что это значит для бота

### 8.1 Что переиспользуем как есть / с доработкой

| Часть | Как есть? | Что мешает (факт кода) |
| --- | --- | --- |
| **Ядро: сигнал, расчёт выгоды, подбор объёма, учёт, риски, застрявшие монеты, перегон USDT** | логика — да | тип монеты `Asset.bsc_contract: Option<alloy Address>` (20 байт) — 29 из 297 файлов ядра импортируют alloy; «газ = BNB» (`core/convert.rs`); хеш транзакции `B256` (32 байта) в `core/types/send.rs` — подпись Solana 64 байта; затраты `*_bnb`. Подробный список — `code-multinet.md` |
| **Шлюз (контракт `Venue`)** | сигнатуры общие | DEX-методы заточены под EVM: исполнитель ArbExecutor (`dex_executor_deploy`), очередь nonce (`wallet_queue`), «сухая сделка eth_simulateV1»; на Solana их смысл другой (нет nonce — есть срок blockhash; нет своего контракта — атомарность транзакции) |
| **Адаптеры бирж (14)** | стаканы, ордера, балансы — да | разбор адреса монеты (`contractAddress` → EVM Address) и имя сети (`venue.bsc_network`) — менять в каждом |
| **Интерфейс** | экраны настроек, сделок, монет, «Упущено» — да | экраны DEX (пулы, каналы, исполнитель, газ BNB) — переделывать под Solana |
| **Настройки, секреты, Telegram, сырой журнал, отчёты** | да | ключ кошелька — ed25519 (не secp256k1) |

**Главное решение для Тиграна:** тип адреса монеты в ядре. Вариант «один экземпляр — одна сеть» не отменяет смены
типа: экземпляр Solana должен хранить 32-байтовый mint. Без этого монета с биржи и пул на DEX не сопоставятся
(правило решения 211). Это **сквозная правка** (по `code-multinet.md`: 193 строки `bsc_contract` в 139 файлах + тесты).

### 8.2 Что заново

**Адаптер `solana_dex` — целиком новый** (на BSC это 428 файлов): связь с узлом (JSON-RPC + WS + gRPC), справочник
пулов (getProgramAccounts / API DEX), разбор аккаунтов каждого вида пула, математика, сборка транзакций (инструкции,
ALT или v1, compute budget, blockhash), кошелёк ed25519 и токен-аккаунты (ATA — нужно создать и оплатить ренту на
каждую монету), проверки токенов (раздел 6), каналы отправки (RPC, Jito, Helius Sender), выяснение исхода по подписи.

### 8.3 Rust-библиотеки (официальные)

| Что | Крейт | Источник |
| --- | --- | --- |
| Базовые типы, транзакции, ключи | `solana-sdk` (и компоненты `solana-transaction`, `solana-keypair`, `solana-pubkey`, `solana-message`, `solana-signature`), `solana-client`, `solana-commitment-config` | [solana.com/docs/clients/official/rust](https://solana.com/docs/clients/official/rust) |
| Токены, compute budget, ATA | `spl-token-interface`, `spl-token-2022-interface`, `spl-associated-token-account-interface`, `solana-compute-budget-interface`, `solana-system-interface` | там же |
| Geyser gRPC | `yellowstone-grpc-client` | [docs.rs](https://docs.rs/yellowstone-grpc-client) |
| Orca | `orca_whirlpools_core` (математика и котировки), `orca_whirlpools_client` (аккаунты, инструкции), совместимы с Solana SDK ^3 | [docs.orca.so](https://docs.orca.so/developers/sdks/overview) |
| Meteora DLMM | крейт `commons` из репозитория `dlmm-sdk` (подключается путём, на crates.io — не найдено) | [docs.meteora.ag](https://docs.meteora.ag/developer-guides/dlmm/rust-integration/library) |
| Manifest | `manifest-dex` | [docs.rs](https://docs.rs/crate/manifest-dex/2.0.0) |
| Raydium, PumpSwap, Jito | официальных Rust-крейтов для котировки **не найдено** (у Raydium исходники программ на GitHub; для Jito на crates.io есть сторонние `jito-sdk-rust` и др. — не официальные, не проверены) | — |
| anchor | Orca client совместим с `anchor >= 0.31` | [docs.orca.so](https://docs.orca.so/developers/sdks/overview) |

### 8.4 Риски

1. **Prop AMM** (~47% объёма по Blockworks/SolanaFloor) — закрыты; без Jupiter их цены не видны, а Jupiter —
   HTTP с лимитами.
2. **Неудачная транзакция платит** (вне бандла) — промахи стоят денег; на BSC через приватные каналы — так же
   отчасти, но здесь это правило сети.
3. **«Uncled blocks»** — бандл Jito может попасть в сеть без атомарности (документация Jito) → нужны проверки
   «минимум на выходе» внутри транзакции.
4. **Токены с freeze authority / permanent delegate / transfer hook** — монету могут заморозить или забрать.
5. **Сеть меняется быстро**: октябрь 2026 — слот 200 мс и новый консенсус; формат транзакций v1 с другими правилами
   (без ALT, лимиты по умолчанию 0) — документацию перечитывать перед работой.
6. **Расхождение данных о долях DEX** у агрегаторов — приоритеты DEX ставить по своим данным.
7. **Биржи**: подтверждений 50–200, реальное время зачисления не известно; у MEXC было приостановление вывода по
   Solana (только заголовок, детали не найдены).
8. **Чужой узел**: getProgramAccounts дорогой и лимитирован (Helius 5–75/с), gRPC — от $499/мес (Helius, QuickNode)
   или по трафику (Triton $0,08/ГБ, Alchemy $75/ТБ, Chainstack $49–149).

### 8.5 Сложность по частям [вывод, оценка]

| Часть | Сложность | Почему |
| --- | --- | --- |
| Смена типа адреса монеты (ядро, шлюз, 14 бирж, UI, SQLite) | высокая по объёму, низкая по смыслу | сквозная, много файлов; логика не меняется |
| Сеть Solana в адаптерах бирж (имя сети, адрес mint) | низкая | настройка + разбор base58 |
| Связь с узлом (RPC, WS, gRPC), blockhash, подтверждения | средняя | новый транспорт, но схема как у BSC |
| Кошелёк ed25519, ATA, рента, балансы SPL | средняя | |
| Справочник пулов + разбор аккаунтов Raydium (3 вида), Orca, Meteora (2–3 вида), PumpSwap | **высокая** | 7+ разных раскладок аккаунтов |
| Математика CLMM/DLMM/CPMM | средняя | идеи есть (V2/V3/Bin), числа и комиссии новые; есть крейты Orca/Meteora для сверки |
| Jupiter `/build` для биржа ↔ DEX | **низкая–средняя** | HTTP-адаптер как KyberSwap на BSC; покрывает prop AMM |
| Проверки токенов (Token-2022, freeze) | низкая | чтение mint |
| Исполнение: транзакция из нескольких свопов, Jito / Sender, выяснение исхода | средняя | атомарность встроена, свой контракт не нужен для простого цикла |
| DEX ↔ DEX циклы | высокая | только свои пулы, prop AMM недоступны |

**Порядок [вывод]:** (1) решение по типу адреса монеты; (2) биржи + Jupiter `/build` → биржа ↔ DEX без своих
пулов; (3) свои пулы Orca/Raydium CLMM/Meteora DLMM по gRPC для потока цен; (4) DEX ↔ DEX.

---

## 9. Чего не нашёл (проверить отдельно)

- Официальный анонс Anza об Alpenglow и слоте 200 мс (есть только провайдеры/СМИ).
- Живое значение чаевых Jito (tip_floor) — запрос не разрешён в этой сессии.
- Описание параметра `accounts` у `simulateTransaction` (возврат состояния или подмена — на странице не сказано).
- Лимиты публичного RPC (числа).
- Размер bin array Meteora, формула свопа DAMM v2 и поддержка Token-2022 в DAMM v2.
- Вид DEX Scorch, AlphaQ, Aquifer, Whalestreet (prop AMM или нет).
- Подтверждение на странице Jupiter фразы о круговых маршрутах (видна только в сводном индексе).
- Поддержка Solana, комиссии и время зачисления у Bybit, Bitget, OKX, XT, HTX, CoinEx, Kraken, Phemex, Poloniex,
  Binance, MEXC — проверить живым справочником сетей адаптеров.
- REST-API пулов Raydium / Meteora / Orca (для справочника пулов) — не исследовались.
- Доля стейка Solana по городам (официальный источник).
