# tools/arbitrum_record — запись ответов своего узла Arbitrum One для тестов

Тесты проекта — только на реально записанных ответах (CLAUDE.md). У разработчика нет доступа к узлу Arbitrum, поэтому
каждое семейство пулов (решение 433, задачи T433-F*) даёт скрипт записи: Тигран запускает его со своего сервера,
скрипт пишет JSON и README (блок, хеши) в tests/data/, после записи с тестов снимается #[ignore]. Скрипты только читают
(eth_call, eth_getBlockByNumber): ключей, подписей и транзакций нет. Python 3 + requests; адрес узла — аргумент.

| Скрипт | Что пишет | Тест |
| --- | --- | --- |
| `curve_crypto.py` | Curve Twocrypto-NG / Tricrypto-NG (решение 442): пулы фабрик, геттеры состояния, проверка (get_coins, version, MATH), лесенка get_dy — tests/data/arbitrum_curve_crypto_<дата>/ | tests/arb_curve_crypto_live.rs |
