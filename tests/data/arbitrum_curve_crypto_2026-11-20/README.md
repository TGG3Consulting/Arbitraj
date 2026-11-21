# Curve crypto-пулы Arbitrum One — запись своего узла, 2026-11-20

Скрипт: tools/arbitrum_record/curve_crypto.py (только eth_call / eth_getCode / eth_getBlockByNumber, без ключей).
Узел: chainId 0xa4b1; блок 526004176 (0x4c9c81a1953957d3374be8dd51ca95253a64e666f84401d39ec03c79cb428e91), время блока 1795198046.

Что записано (на одном блоке): ответы на вызовы бота по calldata (pools/load_crypto.rs, load_crypto_v1.rs),
version(), MATH(), get_coins (и get_decimals у v1) фабрики, код (eth_getCode) пулов, реализаций и математики,
decimals() монет v1, balanceOf(pool) монет и eth_getBalance(pool) (holdings),
лесенка get_dy(i, j, dx) в обе стороны (dx — доли баланса, ppm: [1, 100, 1000, 10000, 100000, 500000]).
Пулы фабрик — с конца pool_list. Откат вызова — поле error.
Читают тесты tests/arb_curve_crypto_live.rs (NG) и tests/arb_curve_crypto_v1_live.rs (v1) — снять #[ignore].

## SHA-256 файлов
af66716b095f7e33bb7f9a26cb9e6b2751e397100c5d7ddb654d1466597f3556  crypto_v1.json
675af2422002e424434b878bad90ee90a8344b99e4a5940db9d081e09db30b7a  tricrypto.json
6b422cc638b621d3b04c7992c535b71b475a8350e0463bdab60c82a5ed29fb53  tricrypto_v1.json
a49f6dbb20566f0b37c111404e32c6de423ace4afeb2578a0d6f93fcb1b4b79b  twocrypto.json
