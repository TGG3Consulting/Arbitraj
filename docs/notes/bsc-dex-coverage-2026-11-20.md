# Покрытие всех DEX BSC — таблица (решение 434, 20.11.2026)

Правило CLAUDE.md «Все DEX сети» (Тигран, 20.11.2026): каждый DEX сети — в таблице со статусом; нет семейства или
источника котировок — обязательная задача. Здесь — ВСЕ 140 DEX GeckoTerminal (BSC), все 257 протоколов DefiLlama
категории Dexs с сетью Binance, плюс агрегаторы, OTC и площадки запуска DefiLlama с этой сетью. Код не менялся.

## 1. Данные и что считается фактом

Данные — `tests/data/bsc_dex_catalog_2026-11-19/` (README там же): список GT, DefiLlama /protocols, выгрузка настроек
DEX бота (dex.kind, dex.pool_enabled, dex.factory, dex.quoter), проба узла бота по 67 id без dex.kind (до 2 пулов:
factory(), token0(), token1(); eth_getCode фабрики), всё 19.11.2026 23:30–23:44 UTC.

Факт семейства (по убыванию силы):
1. Решения и записи узла в репо (tests/data/…: решения 273, 280 — фабрика, комиссия).
2. Ответ узла (factory() пула = адрес фабрики; откат вызова) + **селекторы в коде фабрики** (eth_getCode, разбор
   PUSH4; имена селекторов — по реестру ethereum-lists/4bytes, raw.githubusercontent.com). Фабрика V2 включает
   creationCode пары, поэтому интерфейс пары и текст отката `require(…, "X: K")` видны в коде фабрики.
3. Совпадения кода фабрики с фабрикой уже поддержанной DEX в данных нет — кода фабрик подключённых DEX в выгрузке нет;
   этот признак не использовался.

НЕ факт и не использовалось: похожесть имени (кроме сопоставления строк GT ↔ DefiLlama — оно по имени и так и
помечено), константы в байткоде как «комиссия» (числитель/делитель не отделить от других констант — комиссия V2 только
замером, как в решении 280). Текст описания DefiLlama — факт ответа DefiLlama, не контрактов: строки, где класс взят из
описания, помечены «описание DL».

Сеть: из контейнера Claude узел BSC, GeckoTerminal, DefiLlama и эксплореры недоступны (прокси 403) — новых вызовов не
было, только данные архива.

## 2. Признаки «V2-стандарт»

Фабрика: getPair(address,address), allPairs(uint256), allPairsLength(), createPair(address,address); в коде пары
(встроен в фабрику): swap(uint256,uint256,address,bytes), getReserves(), sync(), skim(address); factory()/token0()/
token1() пулов GT отвечают адресами. Шаг V2 исполнителя меняет прямо в паре (решение 280), роутер не нужен. Комиссия
пары — замер (решение 280: наименьшая feePips, с которой пара принимает обмен; на 1 пипс ниже — откат «…: K»).

## 3. Сводка

Статусы: **(а)** подключён (dex.kind задан, pool_enabled = да); **(б)** семейство есть в коде — нужны только
настройки; **(в)** семейства нет в коде — задача T434-F…; **(г)** не пул (RFQ, книга, агрегатор, кривая, межсетевой,
не спот) — задача T434-Q… или причина Тиграна; **(д)** мёртв — пулов нет по ответу GT и TVL BSC у DefiLlama 0/нет;
**(е)** семейство НЕ установлено фактом — нужна проба (задача T434-P…).

| что | (а) | (б) | (в) | (г) | (д) | (е) | всего |
|---|---|---|---|---|---|---|---|
| id GeckoTerminal (раздел 5) | 73 | 37 | 7 | 1 | 8 | 14 | 140 |
| id из настроек вне GT с dex.kind (раздел 6) | 11 | 0 | 0 | 0 | 0 | 0 | 11 |
| DefiLlama Dexs без id бота + части (раздел 7) | 0 | 0 | 3 | 31 | 23 | 90 | 147 |
| DefiLlama: агрегаторы, OTC, запуски (раздел 8) | 0 | 0 | 0 | 5 | 17 | 20 | 42 |

DefiLlama Dexs с сетью Binance — 257: 113 сопоставлены с id бота по имени (их статус — в разделах 5–6), 144 — раздел 7 (+3 строки «часть протокола»).

Проверка полноты: в разделе 5 — 140 строк (все id GT); в разделах 6–8 — все 257 Dexs DefiLlama (сопоставленные с id
бота — в колонке DefiLlama разделов 5–6, остальные — раздел 7) и 42 протокола других категорий.

Оговорка по (а): выгрузка содержит только dex.kind / pool_enabled / factory / quoter. Заданы ли остальные обязательные
настройки вида (required() в families.rs: роутер, tick_lens, комиссия V2 и т. д.), по выгрузке не проверить — бот
пишет причину «не задана …» в журнал classify.

## 4. (б) — настройки на «да» Тиграну

Полностью готовы (все значения — факт): `orangedx`, `magicfox`, `veplus`, `lif3-v3-bsc`. У остальных одно значение
помечено «измерить» / «НЕ УСТАНОВЛЕНО» — без него вид не подключится (required()).

### 4.1 V2-форки без роутера — `v2_fork_executor` (31)

Общее для каждой строки: `dex.kind` = v2_fork_executor, `dex.v2_fee_source` = fixed, `dex.pool_enabled` = да; `dex.router` не нужен (решение 280). `dex.v2_fee_pct` — только по замеру (метод решения 280); «измерить» — значения в данных нет.

| id DEX | `dex.factory` | `dex.v2_fee_pct` |
|---|---|---|
| `impossible_finance` (Impossible Finance v2) | 0x918d7e714243f7d9d463c37e106235dcde294ffc | измерить |
| `elk_finance_bsc` (Elk Finance (BSC)) | 0x31affd875e9f68cd6cd12cee8943566c9a4bba13 | измерить |
| `yoshi_exchange_bsc` (Yoshi.exchange (BSC)) | 0x542b6524abf0bd47dc191504e38400ec14d0290c | измерить |
| `empiredex_bsc` (EmpireDEX (BSC)) | 0x06530550a48f990360dfd642d2132354a144f31d | измерить |
| `knightswap` (KnightSwap) | 0xf0bc2e21a76513aa7cc2730c7a1d6dee0790751f | измерить |
| `moonlift` (Moonlift) | 0xe9cabbc746c03010020fd093cd666e40823e0d87 | измерить |
| `justmoney_bsc` (Justmoney (BSC)) | 0xf2fb1b5be475e7e1b3c31082c958e781f73a1712 | измерить |
| `radioshack_bsc` (RadioShack (BSC)) | 0x98957ab49b8bc9f7ddbcfd8bcc83728085ecb238 | измерить |
| `swych` (SWYCH) | 0x80f112cd8ac529d6993090a0c9a04e01d495bfbf | измерить |
| `pinkswap` (Pinkswap) | 0x7d2ce25c28334e40f37b2a068ec8d5a59f11ea54 | измерить |
| `spice_trade_bsc` (Spice Trade (BSC)) | 0x19813b4aa783a4aeef5f9155875af2492a2a207d | измерить |
| `cakewswap_bsc` (CakeWSwap (BSC)) | 0x8a521189bfedf99b1dc84bc21571cf8b799dbd4d | измерить |
| `corgiswap` (CorgiSwap) | 0x632f04bd6c9516246c2df373032abb14159537cd | измерить |
| `niob` (Niob) | 0xe0636f192a88de6f1c9ed1a6a0f265c9775c8596 | измерить |
| `w3swap` (W3Swap) | 0xd04a80baeef12fd7b1d1ee6b1f8ad354f81bc4d7 | измерить |
| `openocean` (OpenOcean) | 0xd76d8c2a7ca0a1609aea0b9b5017b3f7782891bf | измерить |
| `kyotoswap` (KyotoSwap) | 0x1c3e50dbbcd05831c3a695d45d2b5bcd691ad8d8 | измерить |
| `lif3-bsc` (lif3 (BSC)) | 0x3fb1e7d5d9c974141a5b6e5fa4edab0a7aa15c6a | измерить |
| `hkswap` (HKSwap) | 0x510eda2e4a6e559094e82aed878aead6c99345b1 | измерить |
| `stronghands-dex-v2` (StrongHands DEX V2) | 0xe184a4047df6d3fb4ac962a4606d75f5e8e87094 | измерить |
| `dex-on-crypto` (Dex on Crypto) | 0x9904c6d92ed455292f88b58665bc95b7019343c2 | измерить |
| `allinxswap-bsc` (AllInXSwap (BSC)) | 0xb755953681f0b54f59336294f6758ae9e2e3ab5e | измерить |
| `reddex-bsc` (Reddex (BSC)) | 0x6d642253b6fd96d9d155279b57b8039675e49d8e | измерить |
| `orangedx` (OrangeDX) | 0xfef9d38f6105c6df2ca4274507f62b44e1e06235 | 0,25 (факт: tests/data/v2_fork_executor_2026-11-15 — наименьшая принятая комиссия 2500 пипсов, на 1 пипс ниже — откат «Orange: K») |
| `pyreswap-bsc` (PyreSwap (BSC)) | 0x045d720873f0260e23da812501a7c5930e510aa4 | измерить |
| `smbswap-v2-bsc` (SMBSwap V2 (BSC)) | 0x2af5c23798fec8e433e11cce4a8822d95cd90565 | измерить |
| `marswap-bsc` (MarSwap (BSC)) | 0xe19165248159b6cb2a2e35bf398581c777c9979a | измерить |
| `token-dex` (Token DEX) | 0xca3256975a99415ca37057d227428051db1c0ec2 | измерить |
| `tidaldex` (TidalDex) | 0x907e8c7d471877b4742da8aa53d257d0d565a47e | измерить |
| `falcox-swap-bnb` (FalcoX Swap (BNB)) | 0xe20642e8d72babc9beb050fc77c3f52f53754eab | измерить |
| `qom-x` (Qom X) | 0x356037cbc77b3a2b36e0484d96df0de247e66785 | измерить |

### 4.2 Остальные (б) (6)

- **`winery_swap`** (Winery Swap):
  - `dex.kind` = v2_fork_executor
  - `dex.factory` = 0x79c342fddbbf376ca6b4efac7aaa457d6063f8cb
  - `dex.v2_fee_source` = pool
  - `dex.v2_fee_method` = swapFee()
  - `dex.v2_fee_denominator` = НЕ УСТАНОВЛЕНО (нужен исходник пары или замер, как Biswap 1000)
  - `dex.pool_enabled` = да
- **`cone_exchange`** (Cone Exchange) — правило и делитель — как в решении 273: запись getAmountOut пар Cone; чей swapFee() (пары или фабрики) — по коду тоже не разделено:
  - `dex.kind` = solidly_v1
  - `dex.factory` = 0x0efc2d2d054383462f2cd72ea2526ef7687e1016
  - `dex.solidly_fee_source` = pool
  - `dex.solidly_fee_method` = swapFee()
  - `dex.solidly_fee_rule` = НЕ УСТАНОВЛЕНО
  - `dex.solidly_fee_denominator` = НЕ УСТАНОВЛЕНО
  - `dex.pool_enabled` = да
- **`magicfox`** (Magicfox) — все значения — из записи и решения 273 (сверено с getAmountOut пар):
  - `dex.kind` = solidly_v1
  - `dex.factory` = 0xceda3234d7d5b36114d886682a399c6d126a03e0
  - `dex.solidly_fee_source` = factory_by_type
  - `dex.solidly_fee_method_stable` = stableFee()
  - `dex.solidly_fee_method_volatile` = volatileFee()
  - `dex.solidly_fee_rule` = numerator
  - `dex.solidly_fee_denominator` = 10000
  - `dex.pool_enabled` = да
- **`veplus`** (Veplus) — все значения — из записи и решения 273:
  - `dex.kind` = solidly_v1
  - `dex.factory` = 0x5bcd9ee6c31def33334b255ee7a767b6eedcba4b
  - `dex.solidly_fee_source` = pool
  - `dex.solidly_fee_method` = swapFee()
  - `dex.solidly_fee_rule` = numerator
  - `dex.solidly_fee_denominator` = 10000
  - `dex.pool_enabled` = да
- **`lif3-v3-bsc`** (Lif3 V3 (BSC)):
  - `dex.kind` = v3_fork
  - `dex.factory` = 0xc84ab9abccc3f35d5f8bdd328ed7e20bc5b81ce4
  - `dex.quoter_virtual` = да
  - `dex.tick_lens` = 0xD9270014D396281579760619CCf4c3af0501A47C (TickLens Uniswap; им читались тики пулов SheepDex — tests/data/sheepdex_2026-11-14)
  - `dex.pool_enabled` = да
- **`smbswap-v3-bsc`** (SMBSwap V3 (BSC)):
  - `dex.kind` = v3_fork_pancake
  - `dex.factory` = 0xa9b5d4eac94cb98117abdcc0ecbd7731960f91d9
  - `dex.quoter_virtual` = да
  - `dex.tick_lens` = НЕ УСТАНОВЛЕНО в выгрузке (взять то же значение, что у hello-dex-bsc / unchain-x в настройках бота)
  - `dex.pool_enabled` = да

## 5. Все DEX GeckoTerminal (140)

| # | id GT | имя GT | DefiLlama (TVL BSC) | семейство — по фактам | статус | что нужно |
|---|---|---|---|---|---|---|
| 1 | `pancakeswap_v2` | Pancakeswap V2 (BSC) | PancakeSwap AMM ($1 252 134 453) | dex.kind = pancakeswap_v2 (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xcA143Ce32Fe78f1f7019d7d551a6402fC5350c73 | **(а)** pool_enabled = да |  |
| 2 | `jswap_bsc` | Jswap (BSC) | — | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xd654cbf99f2907f06c88399ae123606121247d5c | **(а)** pool_enabled = да |  |
| 3 | `apeswap_bsc` | ApeSwap (BSC) | ApeSwap AMM ($6 825 829) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x0841BD0B734E4F5853f0dD8d7Ea041c241fb0Da6 | **(а)** pool_enabled = да |  |
| 4 | `mdex_bsc` | MDEX (BSC) | MDEX ($1 241 951) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x3CD1C46068dAEa5Ebb0d3f55F6915B10648062B8 | **(а)** pool_enabled = да |  |
| 5 | `biswap` | Biswap V2 | Biswap V2 ($4 438 392) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x858E3312ed3A876947EA49d572A7C42DE08af7EE | **(а)** pool_enabled = да |  |
| 6 | `sushiswap_bsc` | Sushiswap (BSC) | SushiSwap ($245 737) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xc35DADB65012eC5796536bD9864eD8773aBc74C4 | **(а)** pool_enabled = да |  |
| 7 | `firebird_bsc` | Firebird (BSC) | — | Взвешенные пары Firebird FLP: фабрика 0x5de74546…18c8 — getPair(address,address,uint32,uint32), createPair(address,address,uint32,uint32), getWeightsAndSwapFee(address), getSwapFee(), isPair; пара — getTokenWeights(), текст отката «FLP: K». Обычной getPair(address,address) нет. GT стр. 1: пулов 7, наиб. резерв $5 627, объём 24 ч $97 | **(в)** | T434-F-FIREBIRD |
| 8 | `impossible_finance` | Impossible Finance v2 | — | V2-стандарт (признаки — раздел 2); текст отката «StableX: K»; обратный вызов stableXCall. GT стр. 1: пулов 20, наиб. резерв $11 910, объём 24 ч $298 | **(б)** | настройки — раздел 4 |
| 9 | `babyswap` | BabySwap | BabySwap ($1 282 679) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x86407bEa2078ea5f5EB5A52B2caA963bC1F889Da | **(а)** pool_enabled = да |  |
| 10 | `wault_finance` | Wault Finance | — | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xB42E3FE71b7E0673335b3331B3e1053BD9822570 | **(а)** pool_enabled = да |  |
| 11 | `mars_ecosystem` | Mars Ecosystem | — | Своя пара: фабрика 0x6f12482d…f3be (getPair/createPair V2), но swap(uint256,uint256,address) без bytes, fee(address), feeScale(), pause(); текст отката «MarsSwapPair::swap: K» — шаг V2 исполнителя зовёт swap(…,bytes). GT стр. 1: пулов 20, наиб. резерв $51 650, объём 24 ч $281 | **(в)** | T434-F-MARS |
| 12 | `jetswap_bsc` | Jetswap (BSC) | JetSwap ($160 848) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x0eb58e5c8aa63314ff5547289185cc4583dfcbd5 | **(а)** pool_enabled = да |  |
| 13 | `elk_finance_bsc` | Elk Finance (BSC) | Elk ($32 253) | V2-стандарт (признаки — раздел 2); текст отката «Elk: K». GT стр. 1: пулов 20, наиб. резерв $17 997, объём 24 ч $78 | **(б)** | настройки — раздел 4 |
| 14 | `annex_finance_bsc` | Annex Finance (BSC) | — | V2 (текст отката «Annex: K»), но пулы GT этой id — от ДВУХ фабрик: 0x6100af69…e4ea и 0x6a616606…4622; dex.factory — один адрес (K::Address) — пулы второй фабрики проверку не пройдут. GT стр. 1: пулов 7, наиб. резерв $6 653, объём 24 ч $28 | **(в)** | T434-F-MULTIFAC |
| 15 | `autoshark_finance` | AutoShark Finance | AutoShark ($133 050) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xe759Dd4B9f99392Be64f1050a6A8018f73B53a13 | **(а)** pool_enabled = да |  |
| 16 | `julswap` | JulSwap | JulSwap ($309 423) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x553990f2cba90272390f62c5bdb1681ffc899675 | **(а)** pool_enabled = да |  |
| 17 | `leonicornswap` | LeonicornSwap | LeonicornSwap ($2 294) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xEB10f4Fe2A57383215646b4aC0Da70F8EDc69D4F | **(а)** pool_enabled = да |  |
| 18 | `yoshi_exchange_bsc` | Yoshi.exchange (BSC) | Yoshi Exchange ($3 753) | V2-стандарт (признаки — раздел 2); текст отката «UniswapV2: K». GT стр. 1: пулов 19, наиб. резерв $3 035, объём 24 ч $8 | **(б)** | настройки — раздел 4 |
| 19 | `empiredex_bsc` | EmpireDEX (BSC) | EmpireDEX ($1 825 173) | V2-стандарт (признаки — раздел 2); текст отката «Empire: K»; у пары ещё sweep/unsweep, createPair(address,address,uint8,uint256). GT стр. 1: пулов 7, наиб. резерв $796, объём 24 ч $16 | **(б)** | настройки — раздел 4 |
| 20 | `polkaex_bsc` | PolkaEx (BSC) | PolkaEx ($91) | GT: пулов 0 (стр. 1, ответ 200), но DefiLlama даёт TVL BSC > 0 — фабрики нет | **(е)** не установлено | T434-P-POLKAEXBSC |
| 21 | `knightswap` | KnightSwap | KnightSwap Finance ($481 948) | V2-стандарт (признаки — раздел 2); текст отката «Pancake: K». GT стр. 1: пулов 20, наиб. резерв $170 012, объём 24 ч $3 769 | **(б)** | настройки — раздел 4 |
| 22 | `impossible_finance_v3` | Impossible Finance v3 | — | Impossible v3: фабрика 0x4233ad9b…f85f — getPair/createPair V2, но у пары makeXybk(uint32,uint32), makeUni(), calcBoost(), getPairSettings(), updateBoost — пары в режиме xybk считают не по x·y=k. GT стр. 1: пулов 11, наиб. резерв $10 426, объём 24 ч $231 | **(в)** | T434-F-IMPOSSIBLE3 |
| 23 | `moonlift` | Moonlift | MoonLift ($790) | V2-стандарт (признаки — раздел 2); текст отката «Moonlift: K». GT стр. 1: пулов 5, наиб. резерв $196, объём 24 ч $0 | **(б)** | настройки — раздел 4 |
| 24 | `sphynx_swap` | Sphynx Swap | Sphynx ($22 981) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x8ba1a4c24de655136ded68410e222cca80d43444 | **(а)** pool_enabled = да |  |
| 25 | `justmoney_bsc` | Justmoney (BSC) | JustMoney ($38 039) | V2-стандарт (признаки — раздел 2); текста «: K» в коде нет; обратный вызов call(address,uint256,uint256,bytes). GT стр. 1: пулов 20, наиб. резерв $12 930, объём 24 ч $369 | **(б)** | настройки — раздел 4 |
| 26 | `radioshack_bsc` | RadioShack (BSC) | RadioShack ($228 581) | V2-стандарт (признаки — раздел 2); текст отката «UniswapV2: K». GT стр. 1: пулов 20, наиб. резерв $135 208, объём 24 ч $826 | **(б)** | настройки — раздел 4 |
| 27 | `pyeswap_bsc` | PYESwap (BSC) | PYEswap ($1 023) | Своя пара: фабрика 0x7b56573c…2f2c — createPair(address,address,bool), swap(uint256,uint256,uint256,uint256,address,bytes) (6 аргументов), totalFee(), depositLPFee; текст отката «PYESwap: K». GT стр. 1: пулов 14, наиб. резерв $998, объём 24 ч $0 | **(в)** | T434-F-PYE |
| 28 | `bakeryswap` | BakerySwap | BakerySwap ($1 553 564) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x01bF7C66c6BD861915CdaaE475042d3c4BaE16A7 | **(а)** pool_enabled = да |  |
| 29 | `pls2e` | PLS2E | Pls2e ($53 139) | dex.kind = v2_fork_executor (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x0944AB692786D9104AE9a29778285c41C33c0415 | **(а)** pool_enabled = да |  |
| 30 | `winery_swap` | Winery Swap | WinerySwap ($83 672) | V2-стандарт (признаки — раздел 2); текст отката «Winery: K»; у пары swapFee(), devFee(), setSwapFee(uint32); у фабрики setSwapFee(address,uint32). GT стр. 1: пулов 19, наиб. резерв $80 315, объём 24 ч $2 665 | **(б)** | настройки — раздел 4 |
| 31 | `nomiswap` | Nomiswap | Nomiswap ($1 355 541) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xd6715A8be3944ec72738F0BFDC739d48C3c29349 | **(а)** pool_enabled = да |  |
| 32 | `safemoon_swap` | SafeMoon Swap | — | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x4d05D0045df5562D6D52937e93De6Ec1FECDAd21 | **(а)** pool_enabled = да |  |
| 33 | `fstswap` | Fstswap | FstSwap ($11 521 114) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x9A272d734c5a0d7d84E0a892e891a553e8066dce | **(а)** pool_enabled = да |  |
| 34 | `dddx` | DDDX | DDDX ($6 642) | factory() пулов 0x9fa51cc5…, 0x12706dea… — откат; token0/token1 отвечают. Фабрика неизвестна. GT стр. 1: пулов 20, наиб. резерв $1 295, объём 24 ч $15 | **(е)** не установлено | T434-P-DDDX |
| 35 | `swych` | SWYCH | Titano Swych ($667 853) | V2-стандарт (признаки — раздел 2); текст отката «Swych: K». GT стр. 1: пулов 19, наиб. резерв $501 588, объём 24 ч $2 334 | **(б)** | настройки — раздел 4 |
| 36 | `orbitalswap` | OrbitalSwap | OrbitalSwap ($19 434) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x1A04Afe9778f95829017741bF46C9524B91433fB | **(а)** pool_enabled = да |  |
| 37 | `pandora_digital_swap` | Pandora Digital Swap | Pandora ($8 382) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xFf9A4E72405Df3ca3D909523229677e6B2b8dC71 | **(а)** pool_enabled = да |  |
| 38 | `fraxswap_bsc` | Fraxswap (BSC) | Frax Swap ($264 013) | Fraxswap (TWAMM): фабрика 0xf89e6ca0…a011 — пары с executeVirtualOrders, getTwammReserves(), longTermSwapFrom0To1, getAmountOut(uint256,address), fee(); резервы getReserves() без исполнения длинных ордеров не равны резервам обмена. GT стр. 1: пулов 6, наиб. резерв $263 141, объём 24 ч $1 822 | **(в)** | T434-F-FRAX |
| 39 | `bridges` | Bridges | — | не определить — пулов нет. GT: пулов 0 (стр. 1, ответ 200) | **(д)** | — |
| 40 | `whaleswap` | WhaleSwap | WhaleSwap ($1 253) | GT: пулов 0 (стр. 1, ответ 200), но DefiLlama даёт TVL BSC > 0 — фабрики нет | **(е)** не установлено | T434-P-WHALESWAP |
| 41 | `pinkswap` | Pinkswap | PinkSwap ($417 255) | V2-стандарт (признаки — раздел 2); текст отката «Pinkswap: K». GT стр. 1: пулов 18, наиб. резерв $323 267, объём 24 ч $128 | **(б)** | настройки — раздел 4 |
| 42 | `baryon_network` | BaryonSwap | Baryon Network ($13 904) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x03879E2a3944FD601e7638DfCBC9253fb793b599 | **(а)** pool_enabled = да |  |
| 43 | `dooar_bsc` | DOOAR (BSC) | DOOAR V2 ($813 995) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x1e895bFe59E3A5103e8B7dA3897d1F2391476f3c | **(а)** pool_enabled = да |  |
| 44 | `spice_trade_bsc` | Spice Trade (BSC) | — | V2-стандарт (признаки — раздел 2); текст отката «UniswapV2: K»; у фабрики defaultFee(), MAX_FEE(), setDefaultFee(uint256). GT стр. 1: пулов 3, наиб. резерв $2 231, объём 24 ч $5 | **(б)** | настройки — раздел 4 |
| 45 | `kyberswap_elastic_bsc` | Kyberswap Elastic (BSC) | KyberSwap Elastic ($715) | dex.kind = kyber_elastic (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xC7a590291e07B9fe9E64b86c58fD8fC764308C4A | **(а)** pool_enabled = да | только сигнал — шага исполнителя нет (exec_kind.rs) |
| 46 | `cone_exchange` | Cone Exchange | Cone ($20 132) | Solidly: фабрика 0x0efc2d2d…1016 — getPair(address,address,bool), createPair(address,address,bool), isPair(address), swapFee(), setSwapFee(address,uint256); в коде пары — stable(), metadata(), getAmountOut(uint256,address), текст отката «ConePair: K». GT стр. 1: пулов 17, наиб. резерв $12 934, объём 24 ч $397 | **(б)** | настройки — раздел 4 |
| 47 | `alium` | Alium | Alium Swap ($15 049) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xbEAC7e750728e865A3cb39D5ED6E3A3044ae4B98 | **(а)** pool_enabled = да |  |
| 48 | `dao_swap` | DAO Swap | DAO Swap ($44 793) | dex.kind = v2_fork_executor (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x940BEb635cbEeC04720AC97FADb97205676e6aa4 | **(а)** pool_enabled = да |  |
| 49 | `ellipsis_finance` | Ellipsis Finance | Ellipsis Finance ($800 579) | Фабрика пулов Ellipsis 0xf65bed27…5870 (find_pool_for_coins, get_coins, is_meta, get_n_coins, exchange(int128,int128,uint256,uint256)); token0()/token1() пулов — откат. Плоский пул или метапул — не проверено (dex.kind ellipsis = curve_stableswap берёт только пулы из dex.stable_pools). GT стр. 1: пулов 6, наиб. резерв $7 426, объём 24 ч $31 | **(е)** не установлено | T434-P-ELLIPSISF |
| 50 | `planet_finance` | Planet Finance | Blue Planet ($131 446) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xa053582601214FEb3778031a002135cbBB7DBa18 | **(а)** pool_enabled = да |  |
| 51 | `babydogeswap` | BabyDogeSwap | BabyDogeSwap ($1 760 471) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x4693b62e5fc9c0a45f89d62e6300a03c85f43137 | **(а)** pool_enabled = да |  |
| 52 | `nomiswap_stable` | Nomiswap (Stable) | Nomiswap ($1 355 541) | dex.kind = nomiswap_stable (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xc6b7ee49d386bae4fd501f2d2f8d18828f1f6285 | **(а)** pool_enabled = да |  |
| 53 | `kyberswap_classic_bsc` | Kyberswap Classic (BSC) | KyberSwap Classic ($35 949) | dex.kind = kyber_dmm (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x878dfe971d44e9122048308301f540910bbd934c | **(а)** pool_enabled = да |  |
| 54 | `cakewswap_bsc` | CakeWSwap (BSC) | — | V2-стандарт (признаки — раздел 2); текст отката «CakeWSwap: K». GT стр. 1: пулов 1, наиб. резерв $3, объём 24 ч $0 | **(б)** | настройки — раздел 4 |
| 55 | `corgiswap` | CorgiSwap | CorgiSwap ($1 700) | V2-стандарт (признаки — раздел 2); текст отката «CorgiSwap: K». GT стр. 1: пулов 9, наиб. резерв $457, объём 24 ч $13 | **(б)** | настройки — раздел 4 |
| 56 | `dinosaureggs` | DinosaurEggs | Dinosaur Eggs ($599 722) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x73D9F93D53505cB8C4c7f952ae42450d9E859D10 | **(а)** pool_enabled = да |  |
| 57 | `niob` | Niob | Niob Finance ($91 883) | V2-стандарт (признаки — раздел 2); текст отката «NiobSwap: K». GT стр. 1: пулов 13, наиб. резерв $24 002, объём 24 ч $14 | **(б)** | настройки — раздел 4 |
| 58 | `padswap-bsc` | Padswap (BSC) | PadSwap ($86 743) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xB836017ACf10b8A7c6c6C9e99eFE0f5B0250FC45 | **(а)** pool_enabled = да |  |
| 59 | `w3swap` | W3Swap | W3swap ($80 687) | V2-стандарт (признаки — раздел 2); текст отката «W3swap: K». GT стр. 1: пулов 20, наиб. резерв $37 415, объём 24 ч $53 | **(б)** | настройки — раздел 4 |
| 60 | `openocean` | OpenOcean | — | V2-стандарт (признаки — раздел 2); текст отката «OpenOcean: K» (пулы GT этой id — V2-пары, не агрегатор). GT стр. 1: пулов 9, наиб. резерв $43 364, объём 24 ч $175 | **(б)** | настройки — раздел 4 |
| 61 | `thena` | THENA | THENA V1 ($1 223 185) | dex.kind = solidly_v1 (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xAFD89d21BdB66d00817d4153E055830B1c2B3970 | **(а)** pool_enabled = да |  |
| 62 | `kyotoswap` | KyotoSwap | KyotoSwap ($25 270) | V2-стандарт (признаки — раздел 2); текст отката «KyotoSwapPair: K»; у фабрики feeAmount(), setFeeAmount(uint256). GT стр. 1: пулов 13, наиб. резерв $11 427, объём 24 ч $230 | **(б)** | настройки — раздел 4 |
| 63 | `lif3-bsc` | lif3 (BSC) | Lif3 Swap ($2 066) | V2-стандарт (признаки — раздел 2); текст отката «UniswapV2: K»; у фабрики swapFee(address), setSwapFee(address,uint256), setDefaultSwapFee — комиссия может быть своя у пары. GT стр. 1: пулов 3, наиб. резерв $2, объём 24 ч $0 | **(б)** | настройки — раздел 4 |
| 64 | `pancakeswap_stableswap` | Pancakeswap (Stable) | PancakeSwap StableSwap ($3 193 445) | dex.kind = pancake_stableswap (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x25a55f9f2279A54951133D503490342b50E5cd15 | **(а)** pool_enabled = да |  |
| 65 | `traderjoe-bsc` | LFJ (BSC) | Joe DEX ($3 488) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x4f8bdc85E3eec5b9dE67097c3f59B6Db025d9986 | **(а)** pool_enabled = да |  |
| 66 | `traderjoe-v2-bsc` | LFJ V2 (BSC) | Joe V2 ($673) | GT: пулов 0 (стр. 1, ответ 200), но DefiLlama даёт TVL BSC > 0 — фабрики нет | **(е)** не установлено | T434-P-TRADERJOEV2BSC |
| 67 | `archly-bsc` | Archly (BSC) | Archly V1 ($140); Archly V2 ($27) | factory() пула 0x3035f843… — откат; token0/token1 отвечают. Фабрика неизвестна. GT стр. 1: пулов 1, наиб. резерв $2, объём 24 ч $0 | **(е)** не установлено | T434-P-ARCHLY |
| 68 | `definix` | Definix | Definix ($241 142) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x43ebb0cb9bd53a3ed928dd662095ace1cef92d19 | **(а)** pool_enabled = да |  |
| 69 | `uniswap-bsc` | Uniswap V3 (BSC) | Uniswap V3 ($41 120 357) | dex.kind = uniswap_v3 (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xdB1d10011AD0Ff90774D0C6Bb92e5C5c8b4461F7 | **(а)** pool_enabled = да |  |
| 70 | `iziswap-bsc` | iZiSwap (BSC) | iZiSwap ($105 671) | dex.kind = iziswap (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x93bb94a0d5269cb437a1f71ff3a77ab753844422 | **(а)** pool_enabled = да |  |
| 71 | `pancakeswap-v3-bsc` | Pancakeswap V3 (BSC) | PancakeSwap AMM V3 ($571 212 420) | dex.kind = pancakeswap_v3 (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865 | **(а)** pool_enabled = да |  |
| 72 | `traderjoe-v2-1-bsc` | LFJ V2.1 (BSC) | Joe V2.1 ($58 727) | dex.kind = lb_v21 (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x8e42f2f4101563bf679975178e880fd87d3efd4e | **(а)** pool_enabled = да |  |
| 73 | `thena-fusion` | THENA FUSION | THENA FUSION ($506 782) | dex.kind = thena_fusion (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x306F06C147f064A010530292A1EB6737c3e378e4 | **(а)** pool_enabled = да |  |
| 74 | `saitaswap-bsc` | Saitaswap (BSC) | SaitaSwap ($17 600) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x6a6dEa0cD52689819Eb3bF2c15594BeAfB646CB7 | **(а)** pool_enabled = да |  |
| 75 | `hkswap` | HKSwap | — | V2-стандарт (признаки — раздел 2); текст отката «HKSwap: K». GT стр. 1: пулов 1, наиб. резерв $8, объём 24 ч $0 | **(б)** | настройки — раздел 4 |
| 76 | `stronghands-dex-v2` | StrongHands DEX V2 | Stronghands ($16) | V2-стандарт (признаки — раздел 2); текст отката «Shnd: K». GT стр. 1: пулов 8, наиб. резерв $80, объём 24 ч $0 | **(б)** | настройки — раздел 4 |
| 77 | `surge-protocol-bsc` | Surge Protocol (BSC) | — | не определить — пулов нет. GT: пулов 0 (стр. 1, ответ 200) | **(д)** | — |
| 78 | `intercroneswap-bsc` | InterCroneSwap (BSC) | Intercroneswap ($22 226) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xFa51B0746eb96deBC619Fd2EA88d5D8B43BD8230 | **(а)** pool_enabled = да |  |
| 79 | `sushiswap-v3-bsc` | Sushiswap V3 (BSC) | SushiSwap V3 ($39 862) | dex.kind = v3_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x126555dd55a39328F69400d6aE4F782Bd4C34ABb | **(а)** pool_enabled = да |  |
| 80 | `magicfox` | Magicfox | MagicFox Swap ($300) | Solidly V1: пул 0x1dde57…f385 (GT) — в allPairs фабрики 0xceda3234…03e0 (tests/data/solidly_fee_2026-11-14, решение 273); factory() пула — откат (raw_rpc_node.json). GT стр. 1: пулов 5, наиб. резерв $118, объём 24 ч $0 | **(б)** | настройки — раздел 4 |
| 81 | `veplus` | Veplus | Veplus ($26 304) | Solidly V1: пул 0x5a4ee6…d375 (GT) — в allPairs фабрики 0x5bcd9ee6…ba4b (tests/data/solidly_fee_2026-11-14, решение 273); factory() пула — откат. GT стр. 1: пулов 10, наиб. резерв $2 255, объём 24 ч $128 | **(б)** | настройки — раздел 4 |
| 82 | `biswap-v3` | Biswap V3 (Legacy) | Biswap V3 ($305 249) | dex.kind = biswap_v3 (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x7c3d53606f9c03e7f54abddffc3868e1c5466863 | **(а)** pool_enabled = да |  |
| 83 | `acsi-finance` | Acsi Finance | — | dex.kind = balancer_v2 (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xb7238020c331e8cdfd26db299cfd2076df812e6f | **(а)** pool_enabled = да |  |
| 84 | `usdfi` | USDFI | — | dex.kind = solidly_v1 (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xb3863573d9f25e6a84895d4685a408db7a488416 | **(а)** pool_enabled = да |  |
| 85 | `smardex-bsc` | SmarDex (BSC) | SMARDEX AMM ($19 005) | dex.kind = smardex (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xa8ef6fea013034e62e2c4a9ec1cdb059fe23af33 | **(а)** pool_enabled = да |  |
| 86 | `ampleswap-bsc` | AmpleSwap (BSC) | AmpleSwap ($14 180) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x381fEfaDAB5466BFf0e8e96842e8e76A143E8F73 | **(а)** pool_enabled = да |  |
| 87 | `dex-on-crypto` | Dex on Crypto | — | V2-стандарт (признаки — раздел 2); текст отката «Pair: K»; у фабрики totalFee(), setTotalFee(uint256), protocolFee(). GT стр. 1: пулов 1, наиб. резерв $2, объём 24 ч $0 | **(б)** | настройки — раздел 4 |
| 88 | `allinxswap-bsc` | AllInXSwap (BSC) | AllInXSwap ($18) | V2-стандарт (признаки — раздел 2); текст отката «AllInX: K». GT стр. 1: пулов 1, наиб. резерв $9, объём 24 ч $0 | **(б)** | настройки — раздел 4 |
| 89 | `diamondswap-bsc` | DiamondSwap (BSC) | Diamondswap ($1 323) | factory() пула → 0x81a1417c…39ca — прокси Diamond (EIP-2535: facets(), facetAddress(bytes4), diamondCut); функции в фасетах, код фасетов не снят. GT стр. 1: пулов 1, наиб. резерв $731, объём 24 ч $11 | **(е)** не установлено | T434-P-DIAMOND |
| 90 | `creamswap` | CreamSwap | — | не определить — пулов нет. GT: пулов 0 (стр. 1, ответ 200) | **(д)** | — |
| 91 | `biswap-v3-1` | Biswap V3 | Biswap V3 ($305 249) | dex.kind = biswap_v3 (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x4d175f2cfe3e2215c1b55865b07787b751cedd36 | **(а)** pool_enabled = да |  |
| 92 | `melegaswap` | MelegaSwap | Melegaswap ($95 259) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xb7e5848e1d0cb457f2026670fcb9bbdb7e9e039c | **(а)** pool_enabled = да |  |
| 93 | `degendex-bsc` | DegenDex (BSC) | DegenDex ($242) | GT: пулов 0 (стр. 1, ответ 200), но DefiLlama даёт TVL BSC > 0 — фабрики нет | **(е)** не установлено | T434-P-DEGENDEXBSC |
| 94 | `donaswap-v3-bsc` | Donaswap V3 (BSC) | — | не определить — пулов нет. GT: пулов 0 (стр. 1, ответ 200) | **(д)** | — |
| 95 | `woken-exchange-bsc` | Woken Exchange (BSC) | Woken Exchange ($3 107) | GT: пулов 0 (стр. 1, ответ 200), но DefiLlama даёт TVL BSC > 0 — фабрики нет | **(е)** не установлено | T434-P-WOKENEXCHANGEBSC |
| 96 | `lif3-v3-bsc` | Lif3 V3 (BSC) | — | V3 (Uniswap): фабрика 0xc84ab9ab…1ce4 — getPool(address,address,uint24), feeAmountTickSpacing, createPool, enableFeeAmount, parameters(); в коде пула (встроен) — swap(address,bool,int256,uint160,bytes), slot0, tickBitmap(int16), ticks(int24), uniswapV3SwapCallback. GT стр. 1: пулов 3, наиб. резерв $34 386, объём 24 ч $1 | **(б)** | настройки — раздел 4 |
| 97 | `lovely-swap` | Lovely Swap | Lovely Swap ($13) | GT: пулов 0 (стр. 1, ответ 200), но DefiLlama даёт TVL BSC > 0 — фабрики нет | **(е)** не установлено | T434-P-LOVELYSWAP |
| 98 | `orion-bsc` | Orion (BSC) | Orion Pools ($198 426) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xe52ccf7b6ce4817449f2e6fa7efd7b567803e4b4 | **(а)** pool_enabled = да |  |
| 99 | `uniswap-v2-bsc` | Uniswap V2 (BSC) | Uniswap V2 ($2 663 323) | dex.kind = uniswap_v2 (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x8909Dc15e40173Ff4699343b6eB8132c65e18eC6 | **(а)** pool_enabled = да |  |
| 100 | `pancakeswap-v1-bsc` | Pancakeswap V1 (BSC) | PancakeSwap AMM ($1 252 134 453) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xBCfCcbde45cE874adCB698cC183deBcF17952812 | **(а)** pool_enabled = да |  |
| 101 | `reddex-bsc` | Reddex (BSC) | reddex ($179) | V2-стандарт (признаки — раздел 2); текст отката «LiquidXv2: K». GT стр. 1: пулов 2, наиб. резерв $172, объём 24 ч $0 | **(б)** | настройки — раздел 4 |
| 102 | `honorswap` | HonorSwap | — | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x5b19629db8d16d551e9f3faac150fa8a11051b33 | **(а)** pool_enabled = да |  |
| 103 | `coinswap` | CoinSwap | Coinswap Space ($242 439) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xC2D8d27F3196D9989aBf366230a47384010440c0 | **(а)** pool_enabled = да |  |
| 104 | `curve-bsc` | Curve (BSC) | Curve DEX ($52 036) | dex.kind = curve_stableswap_ng (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xd7E72f3615aa65b92A4DBdC211E296a35512988B | **(а)** pool_enabled = да |  |
| 105 | `howswap` | HowSwap | — | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xf4866c04a88cb593374865d73d7cf5b5b3a9c641 | **(а)** pool_enabled = да |  |
| 106 | `agus-dex-bsc` | AGUS DEX (BSC) | Agus ($0) | не определить — пулов нет. GT: пулов 0 (стр. 1, ответ 200) | **(д)** | — |
| 107 | `liquidus-finance-bsc` | Liquidus Finance (BSC) | — | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x80CEe6A5c97b1003f272ba8C759a7c17796C2Cdc | **(а)** pool_enabled = да |  |
| 108 | `orangedx` | OrangeDX | — | V2-стандарт (признаки — раздел 2); текст отката «Orange: K». GT стр. 1: пулов 5, наиб. резерв $474, объём 24 ч $8 | **(б)** | настройки — раздел 4 |
| 109 | `pyreswap-bsc` | PyreSwap (BSC) | PyreSwap ($23 349) | V2-стандарт (признаки — раздел 2); текст отката «UniswapV2: K». GT стр. 1: пулов 20, наиб. резерв $2 526, объём 24 ч $1 | **(б)** | настройки — раздел 4 |
| 110 | `smbswap-v2-bsc` | SMBSwap V2 (BSC) | SMBSwap V2 ($1 980) | V2-стандарт (признаки — раздел 2); текст отката «SMB: K». GT стр. 1: пулов 20, наиб. резерв $398, объём 24 ч $4 | **(б)** | настройки — раздел 4 |
| 111 | `smbswap-v3-bsc` | SMBSwap V3 (BSC) | SMBSwap V3 ($47) | V3 (PancakeSwap): фабрика 0xa9b5d4ea…91d9 — getPool(address,address,uint24), feeAmountTickSpacing, createPool, enableFeeAmount, poolDeployer(), setWhiteListAddress(address,bool) (фабрика PancakeSwap V3; кода пула в фабрике нет — интерфейс пула узлом не проверен). GT стр. 1: пулов 2, наиб. резерв $48, объём 24 ч $0 | **(б)** | настройки — раздел 4 |
| 112 | `marswap-bsc` | MarSwap (BSC) | MARSWAP ($254) | V2-стандарт (признаки — раздел 2); текст отката «MSWAP_LP: K»; skim(address) в коде нет; у пары ещё lpFee(), flatFee(), дивиденды. GT стр. 1: пулов 1, наиб. резерв $254, объём 24 ч $4 | **(б)** | настройки — раздел 4 |
| 113 | `pipiswap-bsc` | PipiSwap (BSC) | — | не определить — пулов нет. GT: пулов 0 (стр. 1, ответ 200) | **(д)** | — |
| 114 | `unchain-x` | Unchain X | Unchain X ($1 036 291) | dex.kind = v3_fork_pancake (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x82fa7b2ce2a76c7888a9d3b0a81e0b2ecfd8d40c | **(а)** pool_enabled = да |  |
| 115 | `uniswap-v4-bsc` | Uniswap V4 (BSC) | Uniswap V4 ($26 399 108) | dex.kind = uniswap_v4 (настройка сервера), вид есть в коде (families.rs spec); фабрика — | **(а)** pool_enabled = да |  |
| 116 | `dexswap-bsc` | dexSWAP (BSC) | — | dex.kind = algebra_integral (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x869479270ff5ff40a9cecd49c004d0698ab4b66a | **(а)** pool_enabled = да |  |
| 117 | `maverick-v2-bsc` | Maverick V2 (BSC) | Maverick V2 ($9 149) | dex.kind = maverick_v2 (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x0A7e848Aca42d879EF06507Fca0E7b33A0a63c1e | **(а)** pool_enabled = да |  |
| 118 | `token-dex` | Token DEX | — | V2-стандарт (признаки — раздел 2); текст отката «Pancake: K». GT стр. 1: пулов 15, наиб. резерв $33, объём 24 ч $0 | **(б)** | настройки — раздел 4 |
| 119 | `tidaldex` | TidalDex | TidalDex ($71 137) | V2-стандарт (признаки — раздел 2); текст отката «AmmPair: K». GT стр. 1: пулов 20, наиб. резерв $56 574, объём 24 ч $529 | **(б)** | настройки — раздел 4 |
| 120 | `squadswap-v2-bsc` | SquadSwap Dynamo (BSC) | SquadSwap Dynamo ($216 019) | dex.kind = v2_fork (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x918Adf1f2C03b244823Cd712E010B6e3CD653DbA | **(а)** pool_enabled = да |  |
| 121 | `squadswap-v3-bsc` | SquadSwap WOW (BSC) | SquadSwap WOW ($153 658) | dex.kind = v3_fork_pancake (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x10d8612D9D8269e322AB551C18a307cB4D6BC07B | **(а)** pool_enabled = да |  |
| 122 | `coinfair-bsc` | Coinfair (BSC) | — | не определить — пулов нет. GT: пулов 0 (стр. 1, ответ 200) | **(д)** | — |
| 123 | `tiktokfun` | TikTokFun | — | dex.kind = v3_fork_pancake (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xad18ed8d918e14e5b424d919fb5c829f4d80a4bd | **(а)** pool_enabled = да |  |
| 124 | `thena-v3` | THENA V3 | THENA INTEGRAL ($1 645 514) | dex.kind = algebra_integral (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x30055f87716d3dfd0e5198c27024481099fb4a98 | **(а)** pool_enabled = да |  |
| 125 | `kewlswap-bsc` | KEWLSwap (BSC) | KEWL EXCHANGE ($17) | factory() пула → 0x5636a64b…7646 — прокси Diamond (EIP-2535); код фасетов не снят. GT стр. 1: пулов 1, наиб. резерв $17, объём 24 ч $0 | **(е)** не установлено | T434-P-KEWL |
| 126 | `pancakeswap-infinity-clmm` | Pancakeswap Infinity (CLMM) | PancakeSwap Infinity ($64 586 914) | dex.kind = pancakeswap_infinity_cl (настройка сервера), вид есть в коде (families.rs spec); фабрика — | **(а)** pool_enabled = да |  |
| 127 | `omni-exchange-v2-bsc` | Omni Exchange V2 (BSC) | Omni Exchange V2 ($5) | dex.kind = v2_fork_executor (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x7d9D51267f7e9e6b46a48E0A75c0086F46777087 | **(а)** pool_enabled = да |  |
| 128 | `omni-exchange-v3-bsc` | Omni Exchange V3 (BSC) | Omni Exchange V3 ($2 103) | dex.kind = v3_fork_pancake (настройка сервера), вид есть в коде (families.rs spec); фабрика 0xd6Ab0566e7E60B67c50AC73ddFf4e3DdcB829EC2 | **(а)** pool_enabled = да |  |
| 129 | `omni-exchange-v4-bsc` | Omni Exchange V4 (BSC) | — | не определить — пулов нет. GT: пулов 0 (стр. 1, ответ 200) | **(д)** | — |
| 130 | `four-meme` | Four.meme | four.meme ($3 444 962) | Кривая запуска four.meme (TokenManager2); factory()/token0()/token1() пулов GT — откат. GT стр. 1: пулов 20, наиб. резерв $9 833, объём 24 ч $193 705 | **(г)** | решение 291: не подключается (причина Тиграна) |
| 131 | `coinhain` | Coinhain | Coinhain ($545) | GT: пулов 0 (стр. 1, ответ 200), но DefiLlama даёт TVL BSC > 0 — фабрики нет | **(е)** не установлено | T434-P-COINHAIN |
| 132 | `ring-exchange-bsc` | Ring Exchange (BSC) | Ring Swap ($0) | V2-пары (фабрика 0x4de602a3…dd9b, текст отката «SwapV2: K») над обёртками Few: пулы GT — fwAMDB/fwWBNB, fwSNDKB/fwWBNB; адрес токена пула ≠ адрес монеты биржи (первичный фильтр CLAUDE.md) — нужен шаг обёртки/развёртки. GT стр. 1: пулов 10, наиб. резерв $15 012 133, объём 24 ч $716 502 | **(в)** | T434-F-RING |
| 133 | `dnax` | DNAX | Dnax ($42 429) | GT: пулов 0 (стр. 1, ответ 200), но DefiLlama даёт TVL BSC > 0 — фабрики нет | **(е)** не установлено | T434-P-DNAX |
| 134 | `hello-dex-bsc` | Hello DEX (BSC) | — | dex.kind = v3_fork_pancake (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x0f9823775b670bd123a6369de2a6e8b30341fdae | **(а)** pool_enabled = да |  |
| 135 | `pancakeswap-infinity-lbamm-bsc` | Pancakeswap Infinity LBAMM (BSC) | PancakeSwap Infinity ($64 586 914) | dex.kind = pancakeswap_infinity_bin (настройка сервера), вид есть в коде (families.rs spec); фабрика — | **(а)** pool_enabled = да |  |
| 136 | `topaz` | Topaz | Topaz CL ($568 967); Topaz AMM ($13 034) | dex.kind = slipstream (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x73DC984D9490286E735548f61dfCCec67Af82ed9 | **(а)** pool_enabled = да | пулы Topaz AMM (Solidly) под этим же id видом slipstream не берутся — T434-F-TOPAZ2 |
| 137 | `forest-protocol` | Forest Protocol | Forest V1 ($26 328); Forest V2 ($0) | factory() пула → 0x8ad812a3…2f81: есть getPair/createPair/allPairs и одновременно buyTokens/sellTokens/slope()/tokensSold()/lastPrice() (кривая) — тип пула не установлен. GT стр. 1: пулов 1, наиб. резерв $3 254, объём 24 ч $0 | **(е)** не установлено | T434-P-FOREST |
| 138 | `falcox-swap-bnb` | FalcoX Swap (BNB) | — | V2-стандарт (признаки — раздел 2); текст отката «Pancake: K». GT стр. 1: пулов 2, наиб. резерв $14 909, объём 24 ч $7 955 | **(б)** | настройки — раздел 4 |
| 139 | `qom-x` | Qom X | Qom X ($37 631) | V2-стандарт (признаки — раздел 2); текст отката «QomX: K». GT стр. 1: пулов 10, наиб. резерв $18 327, объём 24 ч $158 | **(б)** | настройки — раздел 4 |
| 140 | `metric-bsc` | Metric (BSC) | Metric V1 ($5 273 673) | dex.kind = metric_omm (настройка сервера), вид есть в коде (families.rs spec); фабрика 0x2a53833cc95548cf52c7b159110e22d3a9018f32 | **(а)** pool_enabled = да |  |

## 6. DEX из настроек бота, которых нет в списке GeckoTerminal

| id в настройках | dex.kind | pool_enabled | фабрика | DefiLlama (TVL BSC) | статус |
|---|---|---|---|---|---|
| `dodo-dpp` | dodo_dpp | да | 0xd9CAc3D964327e47399aebd8e1e6dCC4c251DaAE | DODO AMM ($5 900 010) | **(а)** |
| `dodo-dsp` | dodo_dsp | да | 0x0fb9815938Ad069Bf90E14FE6C596c514BEDe767 | DODO AMM ($5 900 010) | **(а)** |
| `dodo-dvm` | dodo_dvm | да | 0x790B4A80Fb1094589A3c0eFC8740aA9b0C1733fB | DODO AMM ($5 900 010) | **(а)** |
| `ellipsis` | curve_stableswap | да | — | Ellipsis Finance ($800 579) | **(а)** |
| `lista-v3` | v3_fork | да | 0xcb010ed373523942706F730b89792aA1C1597b20 | Lista DEX ($26 601 315) | **(а)** |
| `nerve` | saddle_stableswap | да | — | Nerve ($880 704) | **(а)** |
| `pancakeswap_infinity_bin` | — | да | — | — | id без dex.kind: по classify (families.rs) пулы с этим id не подключаются; id GeckoTerminal другой |
| `pancakeswap_infinity_cl` | — | да | — | — | id без dex.kind: по classify (families.rs) пулы с этим id не подключаются; id GeckoTerminal другой |
| `pancakeswap_v3` | — | да | 0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865 | — | id без dex.kind: по classify (families.rs) пулы с этим id не подключаются; id GeckoTerminal другой |
| `plearn` | v2_fork_executor | да | 0x4750A965f72E09c434b7a40f1735144c983440D2 | — | **(а)** |
| `sheepdex` | v3_fork | да | 0x571521f8c16f3c4ed5f2490f19187ba7a5a3cbdf | SheepDex ($387 227) | **(а)** |
| `squadswap-deprecated` | v2_fork | да | 0x1d9f43a6195054313ac1ae423b1f810f593b6ac1 | SquadSwap V2 ($2 041) | **(а)** |
| `squadswap-v3-bsc-deprecated` | v3_fork_pancake | да | 0x009c4ef7c0e0dd6bd1ea28417c01ea16341367c3 | SquadSwap V3 ($891) | **(а)** |
| `uniswap_v2` | — | да | 0x8909Dc15e40173Ff4699343b6eB8132c65e18eC6 | — | id без dex.kind: по classify (families.rs) пулы с этим id не подключаются; id GeckoTerminal другой |
| `uniswap_v3` | — | да | 0xdB1d10011AD0Ff90774D0C6Bb92e5C5c8b4461F7 | — | id без dex.kind: по classify (families.rs) пулы с этим id не подключаются; id GeckoTerminal другой |
| `uniswap_v4` | — | да | — | — | id без dex.kind: по classify (families.rs) пулы с этим id не подключаются; id GeckoTerminal другой |
| `woofi` | woofi_v2 | да | — | WOOFi Swap ($369 006) | **(а)** только сигнал — шага WOO нет |

## 7. DefiLlama Dexs (BSC) без id в боте — и части протоколов без своего id

TVL BSC — chainTvls.Binance ответа DefiLlama 19.11. «(е) в GeckoTerminal нет, адресов в данных нет» — справочник GT
пулов этой DEX не отдаёт; фабрику брать из официальной документации DEX и проверять узлом (задача T434-P…).

| DefiLlama | slug | TVL BSC | основание | статус | задача |
|---|---|---|---|---|---|
| PancakeSwap Infinity (часть) | pancakeswap-infinity | $64 586 914 | PancakeSwap Infinity Stable (вид pancake_infinity_stable есть, dex.kind ни у одного id нет) | **(е)** | T434-P-INFSTABLE |
| Lista DEX (часть) | lista-dex | $26 601 315 | Lista StableSwap (вид lista_stableswap есть, dex.kind ни у одного id нет; lista-v3 — v3_fork) | **(е)** | T434-P-LISTA |
| BSCSwap | bscswap | $4 391 341 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-BSCSWAP |
| THORChain DEX | thorchain-dex | $1 523 019 | описание DL: межсетевые обмены THORChain (нативные активы, не пул BSC) | **(г)** | T434-Q-THORCHAIN-DEX |
| KaoyaSwap | kaoyaswap | $1 303 028 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-KAOYASWAP |
| Depth | depth | $1 149 669 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-DEPTH |
| Wombat Exchange | wombat-exchange | $784 526 | описание DL: stableswap Wombat (своя математика; ROADMAP.md:21) | **(в)** | T434-F-WOMBAT-EXCHANGE |
| Aequinox | aequinox | $628 079 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-AEQUINOX |
| GIBXSwap | gibxswap | $571 473 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-GIBXSWAP |
| ElfomoFi | elfomofi | $535 196 | описание DL: «Oracle-based DEX» | **(г)** | T434-Q-ELFOMOFI |
| Textile FX | textile-fx | $459 185 | описание DL: «on-chain FX liquidity network» | **(г)** | T434-Q-TEXTILE-FX |
| ValueDefi | valuedefi | $407 828 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-VALUEDEFI |
| Kipseli | kipseli | $344 625 | описание DL: proprietary AMM | **(г)** | T434-Q-KIPSELI |
| Narwhalswap | narwhalswap | $337 491 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-NARWHALSWAP |
| WardenSwap | wardenswap | $250 958 | описание DL: маршрутизация по чужим пулам | **(г)** | T434-Q-WARDENSWAP |
| PantherSwap | pantherswap | $244 357 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-PANTHERSWAP |
| Paraluni Dex | paraluni-dex | $217 023 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-PARALUNI-DEX |
| PureSwap | pureswap | $215 516 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-PURESWAP |
| HyperJump | hyperjump | $163 484 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-HYPERJUMP |
| Alita Finance | alita-finance | $161 633 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-ALITA-FINANCE |
| Maverick V1 | maverick-v1 | $146 857 | Maverick V1 (в коде только maverick_v2) | **(в)** | T434-F-MAVERICK-V1 |
| BurgerSwap | burgerswap | $146 261 | описание DL: агрегатор DEX и CEX | **(г)** | T434-Q-BURGERSWAP |
| Dexalot DEX | dexalot-dex | $108 513 | описание DL: книга ордеров на своей сети | **(г)** | T434-Q-DEXALOT-DEX |
| YouSwap | youswap | $105 255 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-YOUSWAP |
| IDEX Classic | idex-classic | $96 561 | описание DL не называет тип пула | **(е)** | T434-P-IDEX-CLASSIC |
| Lunarbase | lunarbase | $89 056 | описание DL: Proactive Market Maker | **(г)** | T434-Q-LUNARBASE |
| ButterSwap | butterswap | $79 646 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-BUTTERSWAP |
| Smoothy | smoothy | $76 016 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-SMOOTHY |
| OpenLeverage | openleverage | $74 587 | описание DL: маржинальная торговля — не спот: правило CLAUDE.md «Только спот», не подключается | **(г)** | — |
| Bunicorn | bunicorn | $59 101 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-BUNICORN |
| LatteSwap | latteswap | $54 438 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-LATTESWAP |
| Velo Finance | velo-finance | $52 698 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-VELO-FINANCE |
| MochiSwap | mochiswap | $45 557 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-MOCHISWAP |
| DefiBox | defibox | $43 956 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-DEFIBOX |
| CobraSwap | cobraswap | $40 962 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-COBRASWAP |
| MetaCoin Swap | metacoin-swap | $40 816 | описание DL: книга ордеров | **(г)** | T434-Q-METACOIN-SWAP |
| ShibaNova | shibanova | $39 344 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-SHIBANOVA |
| Busta Exchange | busta-exchange | $38 411 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-BUSTA-EXCHANGE |
| AnchorSwap | anchorswap | $36 828 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-ANCHORSWAP |
| Foodcourt | foodcourt | $32 542 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-FOODCOURT |
| Gravis Finance | gravis-finance | $26 772 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-GRAVIS-FINANCE |
| WOWswap | wowswap | $26 296 | описание DL: торговля с плечом — не спот: правило CLAUDE.md «Только спот», не подключается | **(г)** | — |
| PandaSwap | pandaswap | $25 834 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-PANDASWAP |
| PepeTeam DEX | pepeteam-dex | $23 005 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-PEPETEAM-DEX |
| KokomoSwap | kokomoswap | $20 440 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-KOKOMOSWAP |
| RobiniaSwap | robiniaswap | $20 051 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-ROBINIASWAP |
| PICWE | picwe | $16 699 | описание DL: омничейн-ликвидность | **(г)** | T434-Q-PICWE |
| Gas404Swap | gas404swap | $16 673 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-GAS404SWAP |
| MacaronSwap | macaronswap | $16 630 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-MACARONSWAP |
| SashimiSwap | sashimiswap | $16 361 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-SASHIMISWAP |
| SakePerp | sakeperp | $14 225 | описание DL: перпетуальные контракты — не спот: правило CLAUDE.md «Только спот», не подключается | **(г)** | — |
| BeGlobal Finance | beglobal-finance | $13 725 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-BEGLOBAL-FINANCE |
| CafeSwap | cafeswap | $13 360 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-CAFESWAP |
| Topaz AMM (часть) | topaz-amm | $13 034 | Topaz AMM (Solidly, вид solidly_v2 есть), но GT id topaz = slipstream: один id — один вид (families.rs classify) | **(в)** | T434-F-TOPAZ2 |
| YieldFields | yieldfields | $12 444 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-YIELDFIELDS |
| 0.exchange | 0.exchange | $12 413 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-0.EXCHANGE |
| Burve Protocol | burve-protocol | $10 280 | описание DL: кривая запуска (bonding curve) | **(г)** | T434-Q-BURVE-PROTOCOL |
| MCDEX | mcdex | $9 226 | описание DL: перпетуалы, работа прекращена — не спот: правило CLAUDE.md «Только спот», не подключается | **(г)** | — |
| Skate AMM | skate-amm | $8 668 | описание DL: общий пул разных сетей | **(г)** | T434-Q-SKATE-AMM |
| Swipe | swipe | $8 644 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-SWIPE |
| BambooDeFi | bamboodefi | $7 773 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-BAMBOODEFI |
| Dino.Exchange | dino.exchange | $7 526 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-DINO.EXCHANGE |
| Raindex | raindex | $7 066 | описание DL: стратегии-ордера Rainlang | **(г)** | T434-Q-RAINDEX |
| Balanced Exchange | balanced-exchange | $6 852 | описание DL: межсетевая биржа | **(г)** | T434-Q-BALANCED-EXCHANGE |
| Shibance | shibance | $6 810 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-SHIBANCE |
| Spartan | spartan | $6 669 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-SPARTAN |
| TiFi | tifi | $6 518 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-TIFI |
| CrowdSwap | crowdswap | $6 434 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-CROWDSWAP |
| Goblins AMM | goblins-amm | $6 138 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-GOBLINS-AMM |
| Morpheus | morpheus | $6 074 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-MORPHEUS |
| KwikSwap | kwikswap | $6 048 | описание DL: межсетевой протокол | **(г)** | T434-Q-KWIKSWAP |
| RBX | rbx | $5 771 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-RBX |
| Native Swap | native-swap | $4 795 | описание DL: PMM-цены, книга с автоподписью (RFQ) | **(г)** | T434-Q-NATIVE-SWAP |
| SwapFish | swapfish | $4 672 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-SWAPFISH |
| Fynd AMM | fynd-amm | $3 962 | описание DL: расширение пула с динамической комиссией | **(г)** | T434-Q-FYND-AMM |
| KACO Finance | kaco-finance | $3 860 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-KACO-FINANCE |
| FNDZ | fndz | $3 718 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-FNDZ |
| UniFi | unifi | $2 539 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-UNIFI |
| Orion Liquidity Nodes | orion-liquidity-nodes | $2 454 | описание DL: котировки 1:1 с CEX | **(г)** | T434-Q-ORION-LIQUIDITY-NODES |
| Complus Network | complus-network | $2 112 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-COMPLUS-NETWORK |
| ArenaSwap | arenaswap | $1 986 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-ARENASWAP |
| CryptoSwap | cryptoswap | $1 888 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-CRYPTOSWAP |
| BXH | bxh | $1 506 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-BXH |
| TendieSwap | tendieswap | $1 455 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-TENDIESWAP |
| BitBlocks Finance V2 | bitblocks-finance-v2 | $983 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-BITBLOCKS-FINANCE-V2 |
| Litx | litx | $911 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-LITX |
| Octane Finance | octane-finance | $877 | описание DL: лимитные ордера | **(г)** | T434-Q-OCTANE-FINANCE |
| BBBFi Swap V2 | bbbfi-swap-v2 | $868 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-BBBFI-SWAP-V2 |
| BenSwap | benswap | $785 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-BENSWAP |
| Oni Exchange | oni-exchange | $745 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-ONI-EXCHANGE |
| ChampagneSwap | champagneswap | $704 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-CHAMPAGNESWAP |
| Bridgers | bridgers | $688 | описание DL: межсетевой обмен | **(г)** | T434-Q-BRIDGERS |
| PanKUKU | pankuku | $673 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-PANKUKU |
| PattieSwap | pattieswap | $452 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-PATTIESWAP |
| SquadSwap Thanos | squadswap-thanos | $432 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-SQUADSWAP-THANOS |
| AfraSwap | afraswap | $410 | описание DL: агрегирует AMM | **(г)** | T434-Q-AFRASWAP |
| Kapinus | kapinus | $355 | описание DL: фиат-реплики по внутреннему P2P-курсу | **(г)** | T434-Q-KAPINUS |
| COMDEX Exchange | comdex-exchange | $350 | описание DL: цены оракула Pyth | **(г)** | T434-Q-COMDEX-EXCHANGE |
| BitBlocks Finance V3 | bitblocks-finance-v3 | $314 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-BITBLOCKS-FINANCE-V3 |
| OneSwap | oneswap | $199 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-ONESWAP |
| BSCStation | bscstation | $179 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-BSCSTATION |
| Solana Farm | solana-farm | $178 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-SOLANA-FARM |
| Biokript | biokript | $150 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-BIOKRIPT |
| SunFlower Finance | sunflower-finance | $138 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-SUNFLOWER-FINANCE |
| Swapsicle V1 | swapsicle-v1 | $55 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-SWAPSICLE-V1 |
| Derify Protocol | derify-protocol | $50 | описание DL: перпетуальные контракты — не спот: правило CLAUDE.md «Только спот», не подключается | **(г)** | — |
| OpenBiSea | openbisea | $45 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-OPENBISEA |
| BrewSwap | brewswap | $38 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-BREWSWAP |
| Swaap Maker V2 | swaap-maker-v2 | $32 | описание DL: алгоритмический маркет-мейкинг | **(г)** | T434-Q-SWAAP-MAKER-V2 |
| BoomSwap | boomswap | $29 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-BOOMSWAP |
| JellyBeanSwap | jellybeanswap | $29 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-JELLYBEANSWAP |
| BabyDoge Algebra | babydoge-algebra | $28 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-BABYDOGE-ALGEBRA |
| PRODUS | produs | $26 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-PRODUS |
| Ignite DEX | ignite-dex | $20 | описание DL: книга ордеров (CLOB) с хранилищами | **(г)** | T434-Q-IGNITE-DEX |
| MetaTdex | metatdex | $16 | описание DL: сведение ордеров | **(г)** | T434-Q-METATDEX |
| Omni Exchange Flux | omni-exchange-flux | $5 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-OMNI-EXCHANGE-FLUX |
| Akronswap | akronswap | $5 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-AKRONSWAP |
| KOLSwap | kolswap | $2 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-KOLSWAP |
| Bunni V2 | bunni-v2 | $1 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-BUNNI-V2 |
| BrownFi V2 | brownfi-v2 | $0 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-BROWNFI-V2 |
| Kebab Finance | kebab-finance | $0 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-KEBAB-FINANCE |
| Gully Core | gully-core | $0 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-GULLY-CORE |
| BBBFi Swap V3 | bbbfi-swap-v3 | $0 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-BBBFI-SWAP-V3 |
| Soy Finance | soy-finance | $0 | в GeckoTerminal нет, адресов в данных нет | **(е)** | T434-P-SOY-FINANCE |
| Wswap | wswap | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| SushiSwap V4 | sushiswap-v4 | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| Incognito | incognito | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| ZKEX | zkex | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| DeepCore | deepcore | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| Bepswap | bepswap | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| Yogi Finance | yogi-finance | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| Bogged Finance | bogged-finance | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| Component | component | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| BunnyPark | bunnypark | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| Position Exchange | position-exchange | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| Brokoli Network | brokoli-network | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| MarginSwap | marginswap | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| ShiroSwap | shiroswap | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| Demodyfi | demodyfi | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| ZeroSwap | zeroswap | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| Polyo Exchange | polyo-exchange | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| Zircon Gamma | zircon-gamma | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| Sushi Trident | sushi-trident | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| Zootopia Finance | zootopia-finance | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| Snowflake Exchange | snowflake-exchange | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| DONASWAP V2 | donaswap-v2 | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |
| Metric V0 | metric-v0 | $0 | DefiLlama: TVL BSC = 0; в GeckoTerminal нет | **(д)** | — |

## 8. Другие категории DefiLlama с сетью Binance

Агрегаторы (4), OTC (3), площадки запуска (35). Площадки запуска — не DEX по категории DefiLlama; торгует ли площадка
спотом на кривой — не установлено (одна задача T434-L на все строки (е)); four.meme — решение 291. Категория
Derivatives (59 протоколов) не включена — правило «Только спот».

| DefiLlama | категория | TVL BSC | статус | задача |
|---|---|---|---|---|
| 1inch Swap | DEX Aggregator | $203 557 | **(г)** | T434-Q-1INCH-SWAP |
| De1 | DEX Aggregator | $75 053 | **(г)** | T434-Q-DE1 |
| Hashflow | DEX Aggregator | $17 630 | **(г)** | T434-Q-HASHFLOW |
| Rubic | DEX Aggregator | $0 | **(г)** | T434-Q-RUBIC |
| Vailox | OTC Marketplace | $11 250 | **(е)** | T434-L |
| Whales Market | OTC Marketplace | $8 320 | **(е)** | T434-L |
| P2Pmoney | OTC Marketplace | $0 | **(д)** | — |
| PinkSale | Launchpad | $121 407 602 | **(е)** | T434-L |
| DxSale | Launchpad | $13 956 726 | **(е)** | T434-L |
| four.meme | Launchpad | $3 444 962 | **(г)** | решение 291 |
| Flap sh | Launchpad | $2 791 845 | **(е)** | T434-L |
| DeepLock | Launchpad | $1 866 351 | **(е)** | T434-L |
| Genius.fun | Launchpad | $85 812 | **(е)** | T434-L |
| IMPOSSIBLE | Launchpad | $80 563 | **(е)** | T434-L |
| Puppyfun | Launchpad | $29 197 | **(е)** | T434-L |
| ChainGPT | Launchpad | $25 897 | **(е)** | T434-L |
| Mint Club V2 | Launchpad | $10 220 | **(е)** | T434-L |
| iDos Games | Launchpad | $8 549 | **(е)** | T434-L |
| Antex | Launchpad | $4 801 | **(е)** | T434-L |
| CreateMyToken | Launchpad | $1 798 | **(е)** | T434-L |
| Bitbond Sales | Launchpad | $1 092 | **(е)** | T434-L |
| basedbid | Launchpad | $476 | **(е)** | T434-L |
| Peddles | Launchpad | $85 | **(е)** | T434-L |
| GraFun | Launchpad | $1 | **(е)** | T434-L |
| FairyLaunch | Launchpad | $0 | **(е)** | T434-L |
| ALPHA Launchpad | Launchpad | $0 | **(е)** | T434-L |
| PowaDAP | Launchpad | $0 | **(д)** | — |
| Mint Club V1 | Launchpad | $0 | **(д)** | — |
| Decubate | Launchpad | $0 | **(д)** | — |
| Polkastarter | Launchpad | $0 | **(д)** | — |
| Kommunitas | Launchpad | $0 | **(д)** | — |
| Seedify | Launchpad | $0 | **(д)** | — |
| Axl Inu | Launchpad | $0 | **(д)** | — |
| Infinity Pad | Launchpad | $0 | **(д)** | — |
| Uplift DAO | Launchpad | $0 | **(д)** | — |
| Oxbull | Launchpad | $0 | **(д)** | — |
| Kickspad | Launchpad | $0 | **(д)** | — |
| Poolz Vesting | Launchpad | $0 | **(д)** | — |
| Finceptor | Launchpad | $0 | **(д)** | — |
| Printr | Launchpad | $0 | **(д)** | — |
| hodl.dance | Launchpad | $0 | **(д)** | — |
| Lista Pre-IPO | Launchpad | $0 | **(д)** | — |

## 9. Что не установлено и какие данные нужны

- Комиссия 30 V2-форков (раздел 4.1, «измерить») — замер на узле методом решения 280 (eth_simulateV1 шагом V2
  исполнителя), по одной паре с наибольшим стейбл-резервом на фабрику; Winery — делитель swapFee(); Cone — правило и
  делитель (запись getAmountOut пар, как решение 273); SMBSwap V3 — dex.tick_lens.
- (е) по GT (14): DDDX, Archly — фабрика (factory() откатывает: нужны метаданные пула — stable(), metadata(), и
  адрес фабрики из документации); DiamondSwap, KEWLSwap — facets()/facetAddress(bytes4) прокси Diamond и код фасетов;
  Forest — код пула (кривая или пара); Ellipsis (GT ellipsis_finance) — is_meta / get_n_coins у фабрики для пулов GT;
  8 id с пулами 0 у GT, но TVL > 0 у DefiLlama — адреса фабрик из документации DEX.
- (е) по DefiLlama (90): адреса фабрик/пулов из официальной документации каждой DEX → проба узлом (factory(),
  eth_getCode фабрики) → класс по этой же таблице. Без адресов семейство не установить.
- Агрегаторы и описание-DL строки (г): решение Тиграна — свой источник котировок (API площадки) или причина не
  подключать.

