# Nerve 3pool (saddle_stableswap) — сырые ответы узла BSC, 15.11.2026 (решение 292)

Записано Claude 15.11.2026 (2026-11-15T11:05:12Z) из своей вкладки браузера: JSON-RPC eth_call к
https://bsc-rpc.publicnode.com, только чтение, ОДИН блок 133724547 (0x7f87983) — общая проба с
tests/data/ellipsis_2026-11-15 (id запросов — сквозные).

Адрес пула — официальная документация Nerve: docs.nerve.fi/fundamentals/contract-addresses, строка «3pool Pool»
`0x1B3771a66ee31180906972580adE9b81AFc5fCDc` (домен docs.nerve.fi сейчас не резолвится — NXDOMAIN; прочитан снимок
web.archive.org 20221130001820). Там же BTC Pool `0x6C341938bB75dDe823FAAfe7f446925c66E6270c`, ETH Pool
`0x146CD24dCc9f4EB224DFd010c5Bf2b0D25aFA9C0` (на цепи не проверены) и адреса монет (`coins.json`).

Исходник: Sourcify chain 56 — full match (creation и runtime), контракт Swap (solc 0.6.12). Swap.sol развёрнутого =
nerve-finance/contracts bda7578 Swap.sol (SHA-256 содержимого совпадает); SwapUtils.sol отличается только в
addLiquidity и setDevAddress (см. `source_excerpt.txt`) — математика обмена (getD, getY, calculateSwap) та же.

`probe.json` — запрос и ответ каждого вызова:
- getToken(0..3): BUSD, USDT, USDC; getToken(3) — откат «Out of range» (3 монеты);
- getTokenBalance(0..2), getA() = 800, getAPrecise() = 80000, swapStorage() (swapFee = 2 000 000 из 10^10 = 0,02 %,
  adminFee, lpToken 0xf2511b5e…8c14 = «3Pool LP» документации), paused() = false; decimals() монет = 18;
- calculateSwap(i, j, dx) — все 6 сторон × dx = 1, 1 000, 100 000 монет (10^18 единиц).

Перенос из вкладки: слова ответов переданы без ведущих нулей и дополнены обратно до 32 байт
(скрипт сборки — вне репозитория); swapStorage и ошибки — дословно.

Используется: tests/bsc_dex_stable_list_live.rs — (а) кандидаты пар из ответов getToken, (б) своя математика
Saddle = calculateSwap 18 из 18.
