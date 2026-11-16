# Живые ответы XT, 16.11.2026 (записаны ботом на сервере, сырой журнал)

Последние кадры сырого журнала бота (канал `rest /v4/public/wallet/support/currency`, t = 1794867722225, и
`rest /v4/public/currencies`, t = 1794867723152 — 16.11.2026 ≈ 22:22 UTC): поле `m` — тело ответа как есть, сжато zstd.
Выгрузка серверного ИИ (только чтение, без ключей) — задача t163, решение 365, ревью Н1/Н3.

- `support_currency.json.zst` — GET https://sapi.xt.com/v4/public/wallet/support/currency (407 834 байт).
- `currencies.json.zst` — GET https://sapi.xt.com/v4/public/currencies (915 761 байт).

Факты записи: у всех 1166 сетей поле `depositMinAmount` есть и это число (строк, null и пустых нет); в сети
«BNB Smart Chain» (значение venue.bsc_network [xt] на живом боте) 272 сети, из них `depositMinAmount` > 0 у 6 и `0` у 266;
USDT 0.1, USDC 0.01, WIN 0 (у WIN вывод в BSC закрыт; «0» — минимум 0, решение 369); `depositConfirmations` — целое у всех сетей (BSC — 15).
Тест: tests/xt_deposit_min_t163.rs.
