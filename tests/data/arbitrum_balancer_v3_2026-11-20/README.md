# Balancer V3 на Arbitrum — запись своего узла (решения 441, 452)

Блок 526004176, хеш 0x4c9c81a1953957d3374be8dd51ca95253a64e666f84401d39ec03c79cb428e91, время 0x6b008c5e (2026-11-20), chainId 0xa4b1.
Только eth_call (чтение), без from; ключей и подписей нет. Скрипт — tools/arbitrum_record/balancer_v3.py.
Хранилище 0xbA1333333333a1BA1108E8412f11850A5C319bA9, роутер 0xEAedc32a51c510d35ebC11088fD5fF2b47aACF2E; списки фабрик и крючков, прекомпайлы — meta.json. Пулов: 1.

Файлы: meta.json — блок, адреса, списки, пулы; pool_<адрес>.json — calls (все вызовы, которые бот может сделать к пулу: хранилище, фабрики, виды пулов, крючок, прекомпайлы; to, data, result или error), quotes (Router V3 querySwapSingleTokenExactIn: i, j, сумма, ответ), quotes_out (querySwapSingleTokenExactOut: i, j, точный выход, ответ — вход); geckoterminal_pools.json — ответ GeckoTerminal (если --gt-pages).
Решение 460: страницы GeckoTerminal — до выбора блока (узел хранит только недавнее состояние), блок взят после них.

## SHA-256 файлов
1c30c1ddf6327dd15d153098b609fb44fd3a39d9829cc9b4481295fcab5cd966  geckoterminal_pools.json
90dccc916c2536560d58f413f8b382e4d3d24f3cd1aa2c2f8be53dc0d5ab5311  meta.json
c7cdb92db3a136b57b0111c04c76b7f11c57d15499fc523ce16af6e6fc17e5a6  pool_0x5418a64e0cdb20548acb394f5d00a089baf02161.json
