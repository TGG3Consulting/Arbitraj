# tools/arbitrum_record — запись ответов своего узла Arbitrum для тестов семейств пулов (решение 433)

Скрипты только читают (eth_call, eth_getStorageAt, eth_getCode, eth_getLogs, eth_getBlockByNumber, eth_chainId);
ключей, подписей и транзакций нет. Адрес узла и адреса контрактов DEX — аргументы командной строки. Записи ложатся в
`tests/data/arbitrum_<семейство>_<дата>/` с README (что записано, блок, хеш); тесты на них помечены
`#[ignore = "нет записи: tools/arbitrum_record/<семейство>.py"]`, пока записи нет.

| Файл | Семейство | Что пишет |
| --- | --- | --- |
| `lb_v20.py` | Liquidity Book v2.0 (LFJ V2, вид lb_v20, решение 445) | фабрика v2.0, состояние пулов, все бины, getSwapOut роутера v2.0, слоты условия, логи; тест — tests/bsc_dex_lb_v20_arbitrum.rs |
