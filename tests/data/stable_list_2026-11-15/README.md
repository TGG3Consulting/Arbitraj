# Монеты пулов списка dex.stable_pools и ошибка узла — 15.11.2026 (ревью 292)

Записано Claude 15.11.2026 из своей вкладки браузера: JSON-RPC eth_call к https://bsc-rpc.publicnode.com, только
чтение.

`token_calls.json` — все вызовы «монеты пула», которые делает pools/stable_list.rs (token_calls): Nerve 3pool
`0x1B3771a6…fCDc` getToken(0..31) и Ellipsis 3pool `0x160CAed0…4E76` coins(0..7), на одном блоке 133736969
(0x7f8aa09, 2026-11-15T12:38:25Z). Ответы перенесены дословно по форме: монеты 0..2 — адреса (BUSD, USDT, USDC у
Nerve; BUSD, USDC, USDT у Ellipsis), каждый следующий индекс — откат (у Nerve у всех 29 один и тот же «Out of range» с
data, у Ellipsis у всех 5 — «execution reverted»; проверено в вкладке сравнением ответов).

`node_error.json` — настоящий отказ узла (не откат контракта): тот же getToken(0) на старом блоке 0x7f87983 —
−32602 «Archive requests require a personal token…». Тест берёт его текст как сбой части пачки (try_chunks ставит Err на
все вызовы части — node/call.rs).

Используется: tests/bsc_dex_stable_list_live.rs (stable_list::sort — сбой узла у первого пула не сдвигает ответы
второго).
