# Curve crypto-пулы Arbitrum One — запись своего узла, 2026-11-20

Скрипт: tools/arbitrum_record/curve_crypto.py (только eth_call / eth_getCode / eth_getBlockByNumber, без ключей).
Узел: chainId 0xa4b1; блок 525985014 (0xf20ed4d6281542ddcf8181f95fcb5946a14cc320e0ba19ead91b1e6474da4ef1), время блока 1795192982.

Что записано (на одном блоке): ответы на вызовы бота по calldata (pools/load_crypto.rs, load_crypto_v1.rs),
version(), MATH(), get_coins (и get_decimals у v1) фабрики, код (eth_getCode) пулов, реализаций и математики,
decimals() монет v1, balanceOf(pool) монет и eth_getBalance(pool) (holdings),
лесенка get_dy(i, j, dx) в обе стороны (dx — доли баланса, ppm: [1, 100, 1000, 10000, 100000, 500000]).
Пулы фабрик — с конца pool_list. Откат вызова — поле error.
Читают тесты tests/arb_curve_crypto_live.rs (NG) и tests/arb_curve_crypto_v1_live.rs (v1) — снять #[ignore].

## SHA-256 файлов
d642e43e4f0693c6a6e219043375ddde99b91aa183c74897d0d59931001a9c77  crypto_v1.json
5169f11dc5cc3909a52e596e1721808fa61d2215703cb35bdfd40743dc6de1a9  tricrypto.json
c4c9be6f90b67ab8cd2447589e23cf673996a238bf2b5a476b0d8382e2ba24c7  tricrypto_v1.json
b8db627a3f20767e691ab169c9ba863583c63b8b0bbd0475294074c30377d7e8  twocrypto.json
