# tools/arbitrum_record — запись ответов своего узла Arbitrum One для тестов

Тесты проекта — только на реально записанных ответах (CLAUDE.md). У разработчика нет доступа к узлу Arbitrum, поэтому
скрипт записи пишет Тигран со своего сервера: скрипт кладёт JSON и README (блок, хеши) в tests/data/, после записи с
тестов снимается #[ignore]. Скрипты только читают (eth_call, eth_getLogs, eth_getCode, eth_getBlockByNumber): ключей,
подписей и транзакций нет. Python 3 + requests; адрес узла — аргумент.

Скрипты записи своего узла Arbitrum (Python 3, requests). Каждый пишет JSON-файлы и README.md (блок, хеши SHA-256) в
`tests/data/arbitrum_<семейство>_<дата>/`; тесты на эти файлы стоят с `#[ignore = "нет записи: …"]` — после записи
метку снимают. Адрес узла — аргумент (`--rpc` или иной — см. шапку скрипта); адреса контрактов по умолчанию — из официальных источников (в шапке скрипта).

| Скрипт | Решение | Что пишет | Тест |
| --- | --- | --- | --- |
| `kyber_elastic_v1.py` | 451 (T448-06) | пулы KyberSwap Elastic v1: состояние, тики, QuoterV2, getPool фабрики, логи | `tests/arbitrum_kyber_elastic_v1_live.rs` |
| `saddle.py` | 451 (T448-07) | пулы Saddle: монеты, балансы, A, swapStorage (7 слов), calculateSwap, логи | `tests/arbitrum_saddle_live.rs` |
| `woofi.py` | 451 (T448-08) | WooPPV2.2: слот EIP-1967, селекторы в коде реализации, state() оракула, query | `tests/arbitrum_woofi_live.rs` |
| `sub_dex.py` | 450 (T448-01…05) | KyberSwap Classic DMM / Static Fee / Legacy (события создания, getTradeInfo, getPools, getAmountsOut роутеров), Balancer V2 по фабрикам с версией библиотек (состояние, isPoolFromFactory, queryBatchSwap), iZiSwap (fee2pointDelta, NewPool, реестр pool(x, y, fee)), код Universal Router PancakeSwap v3 — tests/data/arbitrum_sub_dex_<дата>/ | `tests/arbitrum_sub_dex_live.rs` |
