# venues/bsc_dex/exec — исполнение обмена

Прямой обмен в одном пуле (V2, V3, V4) — через Universal Router своей DEX: только в нём есть проверка
реально полученного баланса в той же транзакции (`BALANCE_CHECK_ERC20`, решение 40) и одна схема разрешений (Permit2).
Умный маршрут (решение 38) — через Universal Router PancakeSwap Infinity (`smart.universal_router`): все доли в одном
`execute`, у каждой свой минимум, в конце — проверка баланса на сумму минимумов.

| Файл | Что делает |
| --- | --- |
| `ur.rs` | Команды Universal Router, формат по DEX и версии (Uniswap 2.0 / 2.1.1+), BALANCE_CHECK_ERC20, execute |
| `swap_v2v3.rs` | Входы V2_SWAP_EXACT_IN / V3_SWAP_EXACT_IN |
| `swap_v4.rs` | Вход V4_SWAP / INFI_SWAP: обмен + SETTLE_ALL + TAKE_ALL |
| `plan.rs` | План: недостающие разрешения + execute(обмен, проверка баланса) |
| `txs.rs` | Одна транзакция: каналы, цена и лимит газа, подпись, чаевые, отправка, итог |
| `trade.rs` | Сделка прямого маршрута: пересчёт по живому пулу, худшая цена, минимум, разрешения, обмен; общие шаги для умного |
| `trade_smart.rs` | Сделка умного маршрута: пересчёт долей, худшая цена, сверка с MixedQuoter, один execute |
| `universal.rs` | План умного маршрута для Universal Router PancakeSwap: команда на долю, проверка баланса на сумму минимумов |
| `result.rs` | Итог сделки по квитанции |
| `fill.rs` | Суммы по событиям Transfer |
