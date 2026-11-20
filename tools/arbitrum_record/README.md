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
| `fluid_t1.py` | 440 | Fluid DEX T1 (решение 440): состояние всех пулов резолвера на одном блоке и лесенка обменов (swapIn пула → 0x…dEaD, estimateSwapIn резолвера) — для tests/arbitrum_fluid_t1.rs | `tests/arbitrum_fluid_t1.rs` |
| `balancer_v3.py` | 441 | Решения 441, 452 (T433-F2): пулы Balancer V3 всех видов — все вызовы бота к пулу (хранилище, фабрики из списков, виды пулов, крючок, прекомпайлы) и ответы Router V3 querySwapSingleTokenExactIn / ExactOut на одном блоке; тест — tests/arbitrum_balancer_v3.rs | `tests/arbitrum_balancer_v3.rs` |
| `curve_crypto.py` | 442 | Curve crypto-пулы (решения 442, 453): Twocrypto-NG (v2.1.0 и v2.0.0 — по MATH()), Tricrypto-NG, фабрика crypto v1, tricrypto 2021 — ответы на вызовы бота по calldata, проверка (get_coins, version, MATH, get_decimals), eth_getCode реализаций / математики / пулов, balanceOf и ETH пулов, decimals монет v1, лесенка get_dy — tests/data/arbitrum_curve_crypto_<дата>/ | tests/arb_curve_crypto_live.rs (NG), tests/arb_curve_crypto_v1_live.rs (v1) | `tests/arb_curve_crypto_live.rs`, `tests/arb_curve_crypto_v1_live.rs` |
| `camelot_v2.py` | 443 | Решение 443 (T433-F4): Camelot V2 — пары фабрики (резервы, комиссии входа, stableSwap, множители точности, getPair, код), getAmountOut пары на суммах в обе стороны на одном блоке; логи Sync, FeePercentUpdated, SetStableSwap, PairCreated. Запуск: `python3 tools/arbitrum_record/camelot_v2.py http://УЗЕЛ:8547 --factory 0x6EcCab422D763aC031210895C81787E87B43A652` (из корня репозитория; `pip install requests`) | — |
| `lb_v20.py` | 445 | Liquidity Book v2.0 (LFJ V2, вид lb_v20, решение 445) | фабрика v2.0, состояние пулов, все бины, getSwapOut роутера v2.0, слоты условия, логи; тест — tests/bsc_dex_lb_v20_arbitrum.rs | `tests/bsc_dex_lb_v20_arbitrum.rs` |
| `lb_v22.py` | 446 | Liquidity Book V2.2 — LFJ V2.2 (решение 446, T433-F7): `python3 tools/arbitrum_record/lb_v22.py http://УЗЕЛ:8547 --factory 0xb43120c4745967fa9b93E79C149E66B0f2D6Fe0c` | tests/arb_dex_lb_v22_live.rs | `tests/arb_dex_lb_v22_live.rs` |
