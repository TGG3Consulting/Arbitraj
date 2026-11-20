# tools/arbitrum_record — запись ответов своего узла Arbitrum One для тестов семейств DEX

Скрипты Python 3 (`pip install requests`), только чтение узла (eth_call, eth_getLogs, eth_getStorageAt, eth_getCode,
eth_getBlockByNumber) — без ключей, подписей и отправки. Адрес узла — первый аргумент. Пишут JSON в
`tests/data/arbitrum_<семейство>_<дата UTC>/` с README (что записано, блок, sha256 файлов). Запуск — из корня репозитория.

| Скрипт | Семейство | Тесты |
| --- | --- | --- |
| `lb_v22.py` | Liquidity Book V2.2 — LFJ V2.2 (решение 446, T433-F7): `python3 tools/arbitrum_record/lb_v22.py http://УЗЕЛ:8547 --factory 0xb43120c4745967fa9b93E79C149E66B0f2D6Fe0c` | tests/arb_dex_lb_v22_live.rs |
