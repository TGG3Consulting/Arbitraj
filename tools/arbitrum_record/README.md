# tools/arbitrum_record/

Скрипты записи своего узла Arbitrum (Python 3, requests). Каждый пишет JSON-файлы и README.md (блок, хеши SHA-256) в
`tests/data/arbitrum_<семейство>_<дата>/`; тесты на эти файлы стоят с `#[ignore = "нет записи: …"]` — после записи
метку снимают. Адрес узла — аргумент `--rpc`; адреса контрактов по умолчанию — из официальных источников (в шапке скрипта).

| Скрипт | Решение | Что пишет | Тест |
| --- | --- | --- | --- |
| `kyber_elastic_v1.py` | 451 (T448-06) | пулы KyberSwap Elastic v1: состояние, тики, QuoterV2, getPool фабрики, логи | `tests/arbitrum_kyber_elastic_v1_live.rs` |
| `saddle.py` | 451 (T448-07) | пулы Saddle: монеты, балансы, A, swapStorage (7 слов), calculateSwap, логи | `tests/arbitrum_saddle_live.rs` |
| `woofi.py` | 451 (T448-08) | WooPPV2.2: слот EIP-1967, селекторы в коде реализации, state() оракула, query | `tests/arbitrum_woofi_live.rs` |
