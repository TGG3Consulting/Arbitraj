# Промпт для ИИ на сервере: факты по налогу PIG и SMARS (только чтение)

Разбор: `docs/notes/reflection-tax-2026-11-10.md`. Скопировать текст ниже целиком в сессию ИИ на сервере.

---

Задача: собрать факты по двум reflection-токенам на BSC. ТОЛЬКО ЧТЕНИЕ. Разрешено: JSON-RPC узла
`http://127.0.0.1:8545` — `eth_call`, `eth_getTransactionReceipt`, `eth_getTransactionByHash`, `eth_getLogs`,
`eth_getCode`, `eth_getStorageAt`, `eth_blockNumber`, `eth_simulateV1` (только симуляция, в сеть ничего не уходит,
`validation: false`). Запрещено: `debug_*`, `eth_sendRawTransaction`, любые подписи, любые изменения бота / узла / geth,
перезапуски, запросы к боту кроме GET на `127.0.0.1:8787`. Ничего не выдумывать: каждое число — с запросом, которым
оно получено, и номером блока. Если вызов откатился или метода нет — так и записать (текст ошибки), не подставлять
значение. Все сырые ответы сохранить в `/tmp/reflection-facts/` и прислать итог одним файлом
`/tmp/reflection-facts/REPORT.md`.

Адреса:
- PIG   `0x8850d2c68c632e3b258e612abaa8fada7e6958e5`, пара PIG/WBNB `0x51dcaf423fe39f620a13379cd26821cf8d433308`
- SMARS `0xc0366a104b429f0806bfa98d0008daa9555b2bed`, пара SMARS/WBNB `0xfd94e36c1f84dd91df47600697484ba014862c76`
- WBNB  `0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c`
- кошелёк w2 `0xa9C3CACff416e644Dec51986DFD0E17Cc2132817`
- адрес контракта-исполнителя бота: GET `http://127.0.0.1:8787/api/settings`, ключ `exec.executor_address`
  (записать как есть; нет — так и написать).
- Транзакции продаж: PIG TX1 `0x41a6d1a60fd4c7c1df4708a34dd19810c33f3ab7d091da74d3e6d6510a990bb5`,
  PIG TX2 `0xd84c51f8a105f0c2d331b9d6e78302d2d8d33ca28be602ffaf9e7f722ee75817`,
  SMARS A `0x65087831eaacac921a55a058293da0bb4857972f32c7d725a9992a45353d75e3`,
  SMARS B `0xc70528a278e20d8a2dff0a68fc6554dca8a991e611d4ce588aa82a5ad39859ee`.

## 1. Событие Swap пары против нетто Transfer (главный факт)

Для каждой из четырёх транзакций (`eth_getTransactionReceipt`):
- найти лог пары монеты (адрес пары выше) с topic0 = `0xd78ad95fa46c994b6551d0da85fc275fe613ce37657fb8d5e3d130840159d822`
  (Swap(address,uint256,uint256,uint256,uint256,address) UniswapV2/PancakeV2; если такого topic нет — выписать все topic0
  логов пары как есть); разобрать data на amount0In, amount1In, amount0Out, amount1Out;
- `token0()` пары (`eth_call` селектор `0x0dfe1681`) — какая из сумм — монета;
- нетто Transfer монеты кошелёк → пара (topic0 Transfer(address,address,uint256) =
  `0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef`);
- брутто (amountIn из input транзакции; для SMARS A — из input Universal Router, если не разбирается — написать);
- лог Sync пары (topic0 `0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1`) в этой же транзакции:
  reserve0, reserve1 после;
- резервы пары НА БЛОК ДО транзакции: `eth_call getReserves()` (`0x0902f1ac`) на блок N−1 и `balanceOf(пара)` монеты
  (`0x70a08231` + адрес пары) на блок N−1 — «ожидающий излишек» = balanceOf − резерв монеты на N−1
  (оговорка: в блоке N до этой транзакции могли быть другие; записать индекс транзакции в блоке).
Итог по каждой: брутто, нетто Transfer, amountIn монеты из Swap, (amountIn − нетто), (amountIn − нетто)/брутто в %,
излишек на N−1, (amountIn − нетто − излишек)/брутто в %.

## 2. Списки исключений и параметры (стандарт SafeMoon — НЕ проверенный исходник этих токенов)

Селекторы ниже — функции стандартного контракта SafeMoon (SafeMoon.sol, 2021); у форка их может не быть или они могут
называться иначе. Откат / пустой ответ — записать «нет такой функции (откат: …)», не выдумывать. Для PIG и SMARS,
блок = текущий (записать номер):
Селекторы посчитаны как keccak256(сигнатура)[0..4] (pycryptodome); перед вызовом пересчитать самому и сверить.
- `isExcludedFromReward(address)` `0x88f82020` — для: пары монеты, w2, исполнителя, адреса самого токена, `0x000000000000000000000000000000000000dEaD`;
- `isExcludedFromFee(address)` `0x5342acb4` — для: пары, w2, исполнителя, роутера продажи SMARS B (поле `to` транзакции SMARS B),
  роутера продажи PIG TX1 (поле `to` TX1), Universal Router и Permit2 (GET /api/settings, ключи `dex.universal_router`
  и `dex.permit2` — значения по DEX записать как есть);
- без аргументов: `_taxFee()` `0x3b124fe7`, `_liquidityFee()` `0x6bc87c3a`, `totalSupply()` `0x18160ddd`,
  `uniswapV2Pair()` `0x49bd5a5e`, `uniswapV2Router()` `0x1694505e`, `swapAndLiquifyEnabled()` `0x4a74bb02`,
  `_maxTxAmount()` `0x7d1db4a5`, `owner()` `0x8da5cb5b`, `decimals()` `0x313ce567`;
- `balanceOf` (`0x70a08231`) у: пары, адреса самого токена (накопленная комиссия ликвидности),
  `0x…dEaD`, w2.
- `reflectionFromToken(uint256,bool)` `0x4549b039` с (10^decimals × 1000, false) и (тот же, true) — если есть.

## 3. Решающий опыт: перевод монеты перед продажей меняет вход пары? (eth_simulateV1)

Цель — проверить, что перевод монеты между ДВУМЯ ДРУГИМИ адресами поднимает balanceOf пары выше резерва (отражение
доходит до пары), и на сколько это меняет выход продажи. Для каждой монеты, поверх текущего блока (номер записать):
1. Найти держателя монеты H — адрес без кода (`eth_getCode` = `0x`) с балансом ≥ 10× суммы S ниже: перебрать
   получателей событий Transfer этой монеты за последние ~5000 блоков (`eth_getLogs`, topic0 Transfer выше), исключая
   пару, адрес токена, 0x…dEaD и 0x0. S = 0.1 % резерва монеты в паре (getReserves). Записать H, его баланс, S.
2. `eth_simulateV1`, один блок, `validation: false`, вызовы по порядку (все from = H, кроме указанных):
   a. `getReserves()` пары; b. `balanceOf(пара)` монеты;
   c. `transfer(E, S)` монеты (`0xa9059cbb`), где E = `0x00000000000000000000000000000000000000E1` (пустой адрес);
   d. `getReserves()` пары; e. `balanceOf(пара)` монеты.
   Факт: излишек пары (balanceOf − резерв монеты) до (a,b) и после (d,e) перевода, и прирост излишка / S в %.
3. Две продажи S монеты через тот же роутер, что в ручной продаже (SMARS — поле `to` транзакции SMARS B,
   PIG — поле `to` TX1; дополнительно записать `factory()` пары `0xc45a0155` и `factory()` роутера `0xc45a0155` —
   должны совпасть) функцией
   `swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256,uint256,address[],address,uint256)` (`0x5c11d795`),
   amountOutMin = 1, path = [монета, WBNB], to = H, deadline = время блока + 600:
   - Сим. А (как настоящая продажа): approve роутеру (`0x095ea7b3`, сумма S) → `getAmountsOut(S,[монета,WBNB])` (`0xd06ca61f`) →
     `balanceOf(H)` WBNB → продажа → `balanceOf(H)` WBNB.
   - Сим. Б (как проба бота до решения 252): approve → `transfer(E, S)` монеты от H → `getAmountsOut` →
     `balanceOf(H)` WBNB → продажа → `balanceOf(H)` WBNB.
   Для каждой: квотер Q, пришло G, налог по выходу (Q − G)/Q в %. И событие Swap пары из логов продажи (amountIn
   монеты) — если eth_simulateV1 узла отдаёт логи (`"traceTransfers": false`, логи в calls[i].logs); нет — записать.
4. То же, что п. 3, но вместо перевода в Сим. Б — перевод S монеты от ДРУГОЙ пары этой монеты (как пополнение
   симуляции бота, exec/dry_fund.rs): найти другую пару монеты (`eth_getLogs` фабрик — не обязательно; достаточно
   получателя Transfer с кодом, у которого `getReserves()` отвечает и `token0()`/`token1()` (`0x0dfe1681`/`0xd21220a7`) содержит монету, и это не
   пара выше) — from = та пара, to = H. Нет такой пары — записать.

## 4. Налоги, которые сейчас видит бот

GET `http://127.0.0.1:8787/api/coins` — выписать по PIG и SMARS всё, что относится к налогам (покупка, продажа,
перевод, по пулам, если есть), и время проверки. Только GET.

## Формат отчёта

REPORT.md: разделы 1–4, в каждом — таблица чисел + запросы (метод, to, data, блок) + сырой ответ файлом. В конце —
«Чего не удалось получить и почему» (дословные ошибки). Выводов и гипотез не писать — только факты.
