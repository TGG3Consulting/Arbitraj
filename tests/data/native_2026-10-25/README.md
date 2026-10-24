# tests/data/native_2026-10-25 — живые записи для решения 62 (BNB ↔ WBNB)

`arb_native_responses_2026-10-25.json` — ответы публичного узла BSC `https://bsc-dataseed.bnbchain.org` (браузер Тиграна,
2026-10-25, записал техлид) на вызовы `tests/data/native_calls_2026-10-24.json` (коммит 08f7e44, sha256 сверен), все на
блоке `0x7b94144` («latest» заменён номером): `{about, block, results: [{name, request, response}]}`.
Внутри: код WBNB, `WETH()` роутера PancakeSwap V2 (= WBNB), `getPair(WBNB, USDT)` PancakeSwap V2 (0x16b9…0daE) и
`eth_simulateV1` от 0x8894…D4E3: продажа 0,01 BNB с WRAP_ETH (газ 114083, 5 событий), перевод 5 USDT на Universal Router
(51591), покупка WBNB с UNWRAP_WETH (105248, 5 событий), нативный перевод 0,001 BNB (21000, событий нет).

`bingx_bnb_getall_2026-10-24.json` — записи BNB из настоящих ответов BingX `GET /openApi/wallets/v1/capital/config/getall`
(сырой журнал бота 2026-10-24, `data/raw/bingx`, первая и последняя из 912): BEP20 — `contractAddress` = "", ввод и вывод
открыты; XTERIOB и Opbnb — закрыты.

`arb_bingx_bnb_withdraw_2026-10-25.json` — настоящий вывод 0,01998 BNB с BingX (комиссия 0,0005, BEP20, 2026-10-24
12:10:02, TxID из «Истории вывода» BingX): ответы того же узла на eth_getTransactionByHash и eth_getTransactionReceipt.
Прямой нативный перевод: input "0x", value 0,01948 BNB, gasUsed 21000, событий нет, status 0x1.

`arb_native_responses_2_2026-10-25.json` — ответы того же узла (блок `0x7b9607f`) на `tests/data/native_calls_2_2026-10-25.json`
(коммит 398c008, sha256 сверен): getPool(WBNB, USDT) фабрик V3 PancakeSwap (500, 100) и Uniswap (500, 3000) — все ненулевые;
код Universal Router Uniswap; eth_simulateV1 — продажа 0,01 BNB с WRAP_ETH, перевод 5 USDT, покупка с UNWRAP_WETH через UR
PancakeSwap (148706 / 51591 / 126694 газа) и UR Uniswap (112643 / 51603 / 113846), все status 0x1.

`arb_gecko_wbnb_pools_2026-10-25.json` — GeckoTerminal `GET /api/v2/networks/bsc/tokens/{WBNB}/pools?page=1&include=base_token,quote_token,dex`
(20 пулов): WBNB/USDT — pancakeswap-v3-bsc 0,01% и 0,05%, pancakeswap_v2, uniswap-bsc 0,05%; Infinity CL WBNB/USDT нет.

Тесты — tests/bsc_dex_native_live.rs (узел V2, вывод BingX), tests/bsc_dex_native_v3_live.rs (узел V3, GeckoTerminal),
tests/core_native.rs (BingX config/getall).
