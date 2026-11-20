# tools/arbitrum_record — запись ответов своего узла Arbitrum для тестов видов пулов

Скрипты Python 3 (нужен только `requests`). Только чтение (eth_call, eth_getLogs, web3_sha3) — ключей, подписей и
отправки транзакций нет. Каждый пишет папку `tests/data/arbitrum_<вид>_<дата UTC блока>/` с README (блок, хеши файлов).
После записи — закоммитить папку и снять метку `#[ignore]` с тестов вида (решение 433, задачи T433-F*).

## algebra_dir_fee.py — Algebra V1.9 с направленными комиссиями (решение 444, T433-F5; Camelot V3)

Адреса — docs.camelot.exchange/contracts/arbitrum/one-mainnet («AMM V3»): AlgebraFactory
`0x1a3c9B1d2F0529D97f2afC5136Cc23e58f1FD35B`, Quoter `0x0Fc73040b26E9bC8514fA028D998E73A254Fa76E`.

Хеши кода пула (`--init-hash`) — все значения POOL_INIT_CODE_HASH в истории ветки direction-based-fee
cryptoalgebra/AlgebraV1.9 до a45417e (src/core/contracts/AlgebraFactory.sol; коммит, где значение впервые;
«направленные» — feeZto/feeOtz в base/PoolState.sol, «битмап» — TickTable.sol со сжатием на TICK_SPACING или без):

| Хеш | Коммит | Направленные комиссии | Битмап тиков |
| --- | --- | --- | --- |
| 0x77066034e31fe5172b86f4cf68f7e01dd6954f2e8292e6796c9bf7e99dbae477 | 34c23ce (2023-01-10) | да | сжатый (60) |
| 0xb785aaed76f65b185ee1fe081ef2233b7473b6c47bc008865f64dac83f4982f4 | a3dc950 (2023-01-10; начало аудита Paladin 14.06.2023) | да | сжатый (60) |
| 0xb994c319655d0a2474f63ef4c8c5c9ee051c08ddd25e75c6a36c89e8a2a229e7 | affd004 (2023-06-05, ветка v1-modified) | нет | сырой |
| 0x4e92c1fd81a5c8ce82f5a90742e3376c5b258017e149fd0629068ecc9aab43c6 | 82ef500 (2023-06-05, ветка v1-modified) | нет | сырой |
| 0xac97ff1b66ef7517262cbf16db0195b400714e8ecb499fc86f349e77f50ae89b | 4ada8b0 (2023-06-06) | да | сырой |
| 0xdcb95b809dca000f4ce04aa004b723fef2dbb3704db3c0389699bc70f0e42c79 | ade9e23 (2023-06-06, ветка v1-modified) | нет | сырой |
| 0x0ca66607e4d0ece7798fce9ae13cad1a28be1eee443451573f92ba6e3bcd92b9 | 237524b (2023-06-06; конец аудита Paladin 14.06.2023) | да | сырой |
| 0x5d100e2dbf4b80b9451c29380def0d6c4abeb3db0d166c81fda155a89e106dbd | 4c885a5 (2023-06-13) | да | сырой |
| 0xab9acbcae566bd662fb801895320dc3dd170e64394f78600b969ab128a3a40fd | cd4fd3b (2023-06-14) | да | сырой |
| 0x6c1bebd370ba84753516bc1393c0d0a6c645856da55f5393ac8ab3d6dbc861d3 | c63c18f (2023-06-14) … a45417e | да | сырой |

Аудит «Algebra Protocol Tick Spacing Upgrade» (Paladin, 14.06.2023; docs.camelot.exchange/references/audits, раздел
AMM V3) — сравнение a3dc950f…237524b репозитория cryptoalgebra/Algebra; оба коммита есть в ветке direction-based-fee
AlgebraV1.9. Какой код у пулов Camelot V3 — отвечает только запись (совпадение CREATE2).

Код бота читает СЫРОЙ битмап (load_algebra.rs). Тест `tests/arb_dex_algebra_dir.rs` проверяет, что адрес каждого
записанного пула = CREATE2(poolDeployer, keccak256(abi.encode(token0, token1)), хеш) для хеша версии с направленными комиссиями и сырым битмапом.

Команда (из корня репозитория; адрес узла — свой):

```
python3 tools/arbitrum_record/algebra_dir_fee.py --rpc http://<узел>:8547 \
  --factory 0x1a3c9B1d2F0529D97f2afC5136Cc23e58f1FD35B \
  --quoter 0x0Fc73040b26E9bC8514fA028D998E73A254Fa76E \
  --gecko-dex camelot-v3 --gecko-pools 5 \
  --init-hash 0x77066034e31fe5172b86f4cf68f7e01dd6954f2e8292e6796c9bf7e99dbae477 \
  --init-hash 0xb785aaed76f65b185ee1fe081ef2233b7473b6c47bc008865f64dac83f4982f4 \
  --init-hash 0xb994c319655d0a2474f63ef4c8c5c9ee051c08ddd25e75c6a36c89e8a2a229e7 \
  --init-hash 0x4e92c1fd81a5c8ce82f5a90742e3376c5b258017e149fd0629068ecc9aab43c6 \
  --init-hash 0xac97ff1b66ef7517262cbf16db0195b400714e8ecb499fc86f349e77f50ae89b \
  --init-hash 0xdcb95b809dca000f4ce04aa004b723fef2dbb3704db3c0389699bc70f0e42c79 \
  --init-hash 0x0ca66607e4d0ece7798fce9ae13cad1a28be1eee443451573f92ba6e3bcd92b9 \
  --init-hash 0x5d100e2dbf4b80b9451c29380def0d6c4abeb3db0d166c81fda155a89e106dbd \
  --init-hash 0xab9acbcae566bd662fb801895320dc3dd170e64394f78600b969ab128a3a40fd \
  --init-hash 0x6c1bebd370ba84753516bc1393c0d0a6c645856da55f5393ac8ab3d6dbc861d3 \
  --words 8 --bps 1 --bps 10 --bps 100 --log-blocks 20000 \
  --vq-runtime contracts/out/VirtualQuoter.bin-runtime --out tests/data
```

`--words 8` (слов сырого битмапа в каждую сторону, ≈ ±20% цены), `--bps` (доли баланса входа пула: 0,01%, 0,1%, 1%) и
`--log-blocks 20000` (≈ 1,4 ч при блоке 250 мс) — значения для теста, не настройки бота; при отказе узла по диапазону
eth_getLogs — уменьшить. Другие DEX Arbitrum на Algebra (кандидаты — docs/TASKS.md «Решение 444», вопрос 3): тот же
скрипт со своей фабрикой — совпадение CREATE2 с одним из хешей = пулы этой версии кода.
