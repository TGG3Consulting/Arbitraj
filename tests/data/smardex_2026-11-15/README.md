# SmarDex (smardex-bsc) — сырые ответы узла BSC, 15.11.2026

Записано Claude 15.11.2026 из своей вкладки браузера (JSON-RPC к https://bsc-rpc.publicnode.com, только чтение и
eth_call с подменой кода — в сеть ничего не уходило; GeckoTerminal API v2 — список DEX и пулов; Sourcify v2 — исходники).

`probe.json` (блок 133685263, время блока 1794723017 = Multicall3.getCurrentBlockTimestamp на нём):
- фабрика SmardexFactory 0xA8EF6FEa013034E62E2C4A9Ec1CDb059fE23Af33 — Sourcify match (solc 0.8.17; в составе SmardexPair.sol,
  SmardexLibrary.sol); тот же адрес и блок развёртывания 30036610 — в официальном репозитории
  github.com/SmarDex-Ecosystem/dex-smart-contracts@74d0444 deployments/bsc/SmardexFactory.json; тексты пары, библиотеки и
  фабрики из solcInputs/1a3e5ed8… совпали с Sourcify (SHA-256 без \r). allPairsLength = 57, factory() всех пар = фабрика;
  комиссии — в паре (getPairFees; у 46 пар 9000/1000 над 1e6, у остальных — свои);
- четыре пары (код 19 403 байта, PUSH4 swap 0x1f18b371 и smardexSwapCallback 0xa1dab4eb): getReserves (баланс − доля
  протокола), getFictiveReserves, getPriceAverage, getPairFees, getFeeToAmounts и ответы виртуального квотера quoteSmardex
  (пара исполняет свой swap и откатывает в обратном вызове с дельтами) на 3 суммах (0,01 %, 1 %, 10 % резерва входа) в обе
  стороны — 24 ответа; «взял» = вход во всех;
- id DEX у GeckoTerminal — `smardex-bsc` (16 пулов; все 16 — пары этой фабрики).

`exec_sim.json` — шаг SMARDEX (вид 16) ArbExecutor v2 через eth_call с подменой кода (код решения 288, sha256 байтов
5ea19113dd5362fa…): получено ровно quoteSmardex на том же блоке во всех 6 случаях (4 пары); тот же маршрут видом V3 — откат. Тест сверяет
received с НАШЕЙ формулой (math/smardex_swap.rs) на состоянии пары из probe.json того же блока.

`probe_time.json` (блок 133694132, время блока 1794727010) — ветки, которых нет на блоке probe.json (последние обмены
четырёх пар — за 15 508 с и раньше до блока чтения, обменов в окне 300 с на цепи не найдено): те же 4 пары с eth_call и
переопределением времени блока (4-й параметр geth, blockOverrides.time; состояние — настоящее) на time =
priceAverageLastTimestamp (dt = 0) и + 120 с (0 < dt < 300), обе ветки деления на два шага; 4 пары без записи средней
цены (priceAverageLastTimestamp = 0, ни одного обмена; резервы — единицы–миллионы wei) — без переопределения. 46 ответов
квотера. В 2 из них (пара 0x4460…5071, вход token1) квотер вернул выход 0, а обмен откатился бы: новый фиктивный резерв = 0,
require в _update (SmardexPair.sol L358) идёт ПОСЛЕ обратного вызова, в котором квотер откатывает; тест проверяет, что наша
формула даёт None, а выход getAmountOut равен ответу квотера.

`source_excerpt.txt` — дословные выдержки официального исходника (solcInputs фабрики BSC) с номерами строк: SmardexLibrary
(константы, approxEq, getUpdatedPriceAverage, computeFirstTradeQtyIn, computeFictiveReserves, applyKConstRuleOut,
getAmountOut), SmardexPair (комиссии в паре, геттеры, swap со средней ценой к block.timestamp и обратным вызовом, setFees,
_update), ISmardexSwapCallback, события пары, событие PairCreated (ISmardexFactory L12), createPair фабрики.

Используется: tests/bsc_dex_smardex_live.rs (решение 288).
