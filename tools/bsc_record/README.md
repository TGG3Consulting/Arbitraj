# tools/bsc_record — запись ответов своего узла BSC для тестов

Только чтение узла (eth_call, eth_getCode, web3_sha3), транзакций, ключей и подписей нет. Нужен Python 3.8+
(стандартная библиотека; HTTP — `tools/arbitrum_record/stdhttp.py`, `pip` не нужен). Вопросов в процессе нет; последняя
строка — «Архив для отправки: …zip» — этот zip прислать.

| Скрипт | Решение | Что пишет | Тест |
| --- | --- | --- | --- |
| `thena_fusion.py` | 461, ревью t235 (T461-06) | THENA Fusion (вид thena_fusion): каждый пул на своём свежем блоке — состояние, код и keccak, слова битмапа в двух толкованиях (сырое tick >> 8 — как бот; сжатое на tickSpacing — как AlgebraV1), ticks(), котировки виртуального квотера бота и Quoter THENA на суммах в обе стороны; poolDeployer фабрики, CREATE2, ответы Sourcify — `tests/data/bsc_thena_fusion_<дата>/` | `tests/bsc_thena_fusion_row_live.rs` |
| `rec_rpc.py` | ревью t235 | JSON-RPC узла: вызов, пачка (batch), keccak256 / селекторы через web3_sha3 — модуль `thena_fusion.py`, запускать не нужно | — |

## thena_fusion.py

Из корня репозитория:

- Linux: `python3 tools/bsc_record/thena_fusion.py --rpc http://127.0.0.1:8545`
- Windows: `py tools\bsc_record\thena_fusion.py --rpc http://127.0.0.1:8545`

Адреса — официальная таблица контрактов THENA (docs.thena.fi/thena/official-links, «List of smart contracts»):
AlgebraFactory `0x306F06C147f064A010530292A1EB6737c3e378e4`, Quoter `0xeA68020D6A9532EeC42D4dB0f92B83580c39b2cA`.
Пулы — первая страница GeckoTerminal `thena-fusion-bsc` (`--gecko-pools`, по умолчанию 8) и/или `--pool 0x…`.
Нужен доступ в интернет к api.geckoterminal.com (до записи блоков) и sourcify.dev (после; сбой Sourcify запись не
останавливает). Узел с короткой историей состояния: каждый пул читается тремя пачками на голове, взятой перед пулом.
