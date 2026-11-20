# tools/arbitrum_record — запись ответов своего узла Arbitrum One для тестов

Тесты проекта — только на реально записанных ответах (CLAUDE.md). У разработчика нет доступа к узлу Arbitrum, поэтому
скрипт записи пишет Тигран со своего сервера: скрипт кладёт JSON и README (блок, хеши) в tests/data/, после записи с
тестов снимается #[ignore]. Скрипты только читают (eth_call, eth_getLogs, eth_getCode, eth_getBlockByNumber): ключей,
подписей и транзакций нет. Python 3 + requests; адрес узла — аргумент.

| Скрипт | Что пишет | Тест |
| --- | --- | --- |
| `sub_dex.py` | Решение 450 (T448-01…05): KyberSwap Classic DMM / Static Fee / Legacy (события создания, getTradeInfo, getPools, getAmountsOut роутеров), Balancer V2 по фабрикам с версией библиотек (состояние, isPoolFromFactory, queryBatchSwap), iZiSwap (fee2pointDelta, NewPool, реестр pool(x, y, fee)), код Universal Router PancakeSwap v3 — tests/data/arbitrum_sub_dex_<дата>/ | tests/arbitrum_sub_dex_live.rs |
