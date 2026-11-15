# Ellipsis 3pool (curve_stableswap) — сырые ответы узла BSC, 15.11.2026 (решение 292)

Записано Claude 15.11.2026 (2026-11-15T11:05:12Z) из своей вкладки браузера: JSON-RPC eth_call к
https://bsc-rpc.publicnode.com, только чтение, ОДИН блок 133724547 (0x7f87983) — общая проба с
tests/data/nerve_2026-11-15 (id запросов — сквозные).

Адрес пула `0x160CAed03795365F3A589f10C379FfA7d75d4E76` — официальные репозитории Ellipsis:
ellipsis-finance/stableswap-factory b0079ba, contracts/implementations/meta/MetaUSD.vy L89 (BASE_POOL), монеты —
L91-L95 (BASE_COINS: BUSD, USDC, USDT); ellipsis-finance/ellipsis 6bdf778, scripts/metapool.py L14. В официальной
документации docs.ellipsis.finance (dev/deployment-links) адреса пулов нет — только EPX, IncentiveVoting и т. п.

Исходник: Sourcify chain 56 — full match, Vyper 0.2.11; содержимое = ellipsis-finance/ellipsis 6bdf778
contracts/StableSwap.vy, кроме двух пустых строк (см. `source_excerpt.txt`). N_COINS = 3, RATES = [1e18; 3].
Геттера is_killed у этого кода нет (переменная не public).

`probe.json` — запрос и ответ каждого вызова:
- coins(0..3): BUSD, USDC, USDT; coins(3) — откат;
- balances(0..2), A() = 1500, fee() = 4 000 000 из 10^10 (0,04 %), admin_fee() = 0; decimals() монет = 18;
- get_dy(int128 i, int128 j, dx) — все 6 сторон × dx = 1, 1 000, 100 000 монет.

Перенос из вкладки — как в tests/data/nerve_2026-11-15/README.md.

Используется: tests/bsc_dex_stable_list_live.rs — (а) кандидаты пар из ответов coins, (б) своя математика
Curve 2020 = get_dy 18 из 18.
