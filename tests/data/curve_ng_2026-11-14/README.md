# Curve StableSwap-NG (curve-bsc) — сырые ответы узла BSC, 14.11.2026

`probe.json` — записано Claude 14.11.2026 из своей вкладки браузера (eth_call / eth_getLogs к
https://bsc-rpc.publicnode.com, только чтение) на ОДНОМ блоке 133594967:
- пулы фабрики NG `0xd7E72f3615aa65b92A4DBdC211E296a35512988B`: rtUSQ/USDT `0xc2d55a4c…d563`, WBNB/USDT
  `0x47232815…04cb`, USDT/USDC/FDUSD `0xa5e0e464…8ec4` — N_COINS, A_precise, fee, offpeg_fee_multiplier,
  stored_rates, version, coins(k), balances(k), decimals() монет;
- реестр фабрики: get_coins(pool), is_meta(pool), get_pool_asset_types(pool);
- get_dy(i, j, dx) пула по всем парам индексов на 1, 100, 10000 единиц монеты входа (WBNB — /600);
- последние 6 логов пулов (TokenExchange, topic0 0x8b3e96f2…).
Используется: tests/bsc_dex_curve_ng_live.rs (решение 275).
