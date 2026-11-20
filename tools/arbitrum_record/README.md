# tools/arbitrum_record — запись ответов своего узла Arbitrum One для тестов (решение 433, семейства T433-F*)

Скрипты только читают узел (eth_call, eth_getLogs, eth_getCode, блоки) — без ключей, подписей и отправки. Пишут
JSON-файлы и README с блоком и sha256 в `tests/data/arbitrum_<семейство>_<дата>/`. Тесты семейства помечены
`#[ignore = "нет записи: tools/arbitrum_record/<семейство>.py"]` — после записи метку снимаем.

| Файл | Что делает |
| --- | --- |
| `camelot_v2.py` | Решение 443 (T433-F4): Camelot V2 — пары фабрики (резервы, комиссии входа, stableSwap, множители точности, getPair, код), getAmountOut пары на суммах в обе стороны на одном блоке; логи Sync, FeePercentUpdated, SetStableSwap, PairCreated. Запуск: `python3 tools/arbitrum_record/camelot_v2.py http://УЗЕЛ:8547 --factory 0x6EcCab422D763aC031210895C81787E87B43A652` (из корня репозитория; `pip install requests`) |
