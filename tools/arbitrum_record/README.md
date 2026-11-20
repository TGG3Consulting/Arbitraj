# tools/arbitrum_record — запись ответов своего узла Arbitrum для тестов семейств пулов

Тесты — только на реально записанных ответах узла (CLAUDE.md). Скрипт запускает Тигран на своём сервере (адрес узла —
аргумент), он пишет JSON в `tests/data/arbitrum_<семейство>_<дата>/` с README (блок, хеш, SHA-256 файлов). Только чтение
(eth_call), ключей и подписей нет.

| Файл | Что |
| --- | --- |
| `balancer_v3.py` | Решение 441 (T433-F2): пулы Balancer V3 — неизменяемое, состояние (как pools/load_balancer_v3.rs) и ответы Router V3 querySwapSingleTokenExactIn на одном блоке; тест — tests/arbitrum_balancer_v3.rs |
