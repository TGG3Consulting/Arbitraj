# tools/arbitrum_record — запись ответов своего узла Arbitrum One для тестов

Тесты проекта — только на реально записанных ответах (CLAUDE.md). У разработчика нет доступа к узлу Arbitrum, поэтому
каждое семейство пулов (решение 433, задачи T433-F*) даёт скрипт записи: Тигран запускает его со своего сервера,
скрипт пишет JSON и README (блок, хеши) в tests/data/, после записи с тестов снимается #[ignore]. Скрипты только читают
(eth_call, eth_getBlockByNumber): ключей, подписей и транзакций нет. Python 3 + requests; адрес узла — аргумент.

| Скрипт | Что пишет | Тест |
| --- | --- | --- |
| `curve_crypto.py` | Curve crypto-пулы (решения 442, 453): Twocrypto-NG (v2.1.0 и v2.0.0 — по MATH()), Tricrypto-NG, фабрика crypto v1, tricrypto 2021 — ответы на вызовы бота по calldata, проверка (get_coins, version, MATH, get_decimals), eth_getCode реализаций / математики / пулов, balanceOf и ETH пулов, decimals монет v1, лесенка get_dy — tests/data/arbitrum_curve_crypto_<дата>/ | tests/arb_curve_crypto_live.rs |
