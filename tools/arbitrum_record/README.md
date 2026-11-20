# tools/arbitrum_record — запись фактов узла Arbitrum для тестов семейств пулов (решение 433)

Каждый скрипт — одно семейство пулов: Python 3 + requests, адрес узла — аргумент `--rpc`, пишет JSON-файлы и README
(что записано, блок, SHA-256) в `tests/data/arbitrum_<семейство>_<дата>/`. Только чтение: eth_call, eth_getLogs,
eth_getBlockByNumber; ключей и подписей нет. Тесты на эти файлы помечены `#[ignore = "нет записи: …"]` — после записи
метку снимаем.

| Файл | Семейство | Тесты |
| --- | --- | --- |
| `ekubo.py` | Ekubo (решение 447, T433-F8): пулы GeckoTerminal ekubo-v3-arbitrum и/или `--pool`, ключи PoolKeyIndex, Core.sload (состояние, битмап, тики), MEVCapture.sload, MEVCaptureRouter.quote, логи ядра | tests/arbitrum_ekubo_live.rs |

Запуск: `python3 tools/arbitrum_record/ekubo.py --rpc http://<узел>:8547 --gt`
