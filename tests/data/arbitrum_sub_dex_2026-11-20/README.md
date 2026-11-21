# arbitrum_sub_dex_2026-11-20 — запись tools/arbitrum_record/sub_dex.py (решения 450, 460 п. 9)

Блок 526015961 (0x9126c8095bb51d0f0ab519411444fe7689501ce920ad02bfca34218909a263d7), время блока 1795201210. Только чтение (eth_call, eth_getLogs, eth_getCode,
eth_getBlockByNumber) и GeckoTerminal. Пулы — без скана истории (решение 460 п. 9): KyberSwap Classic — хвост allPools и
GeckoTerminal, событие создания — окном вокруг pool_created_at (kyber.json: pools — с событием, pools_no_log — без,
с причиной); Balancer V2 — GeckoTerminal + isPoolFromFactory; iZiSwap — реестр pool(x, y, fee) по парам токенов.
Поиск пулов уложился в потолок 600 с.
Тест — tests/arbitrum_sub_dex_live.rs (после записи снять #[ignore]).

| Файл | SHA-256 |
| --- | --- |
| kyber.json | 65eea46419b6ce0076a5ff828042d0d647191143dda0d250f3c91bc326d0cd22 |
| balancer.json | 484f70c33e3851ad509c7aba95d81467d07b5017ff86b34208b84505711b5649 |
| izi.json | e1632ea0203c2d97249bc73ba61ecaf5ee04aca48a71f1dc5375adea1a646dde |
| pancake_ur.json | 42934ea4d19f65f7380fda3f2d1bf4f36a4d6ecddb79622e9559b434df4592ef |
| block.json | 0fe40e0b354bc5d9e6fc76fe25927476ed27512b3de2996254f25a564d5d5031 |
