# Запись: V2-форки без роутера — обмен через код ArbExecutor (решение 280)

Снято 15.11.2026 (UTC 14.11) через Chrome владельца, `fetch` к `https://bsc-rpc.publicnode.com`, все вызовы — на ОДНОМ
блоке **133619188** (`0x7f6ddf4`). Только чтение (eth_call, eth_getCode, eth_simulateV1 — в сеть ничего не уходит).

Код исполнителя: `eth_getCode(0x9ef558a5d437718dde44e00990e0efb9976cf794)` (исполнитель кошелька проб из настроек
владельца) — SHA-256 hex-строки совпал с `contracts/out/ArbExecutor.bin-runtime`; он и подставлялся в симуляцию.
Адрес подмены `0x09232102…713c` = keccak256("arb.v2_pair_quote")[12..], владелец `0x914416a6…a18b` =
keccak256("arb.v2_pair_quote.owner")[12..] (как quote/v2_pair_quote.rs), слот owner = 0 (ArbExecutor.layout.json).

Пара каждой DEX: фабрика → `allPairsLength` / `allPairs(i)` → `getReserves`, `token0`, `token1`; выбрана пара с
наибольшим резервом USDT / BUSD / USDC; `factory.getPair(token0, token1)` = пара (поле `getPair`). Вход — стейбл пары,
сумма = 1% его резерва (`amount_in`).

Симуляция — ровно вызовы quote/v2_pair_quote.rs `calls`: 0 getReserves → 1 пара переводит вход владельцу подмены →
2 sync() → 3 getReserves → 4 approve исполнителю → 5 execute(вход, сумма, выход, 1, uint256.max, [шаг V2, feePips]).
Комиссия — поиск делением: наименьшие `feePips`, при которых шаг 5 проходит (`min_fee_pips`). `ok` — итоги при ней
(status / returnData / gasUsed; события опущены), `below_1pip_last` — итог шага 5 при `min_fee_pips − 1` (шаги 0–4
совпали с `ok` побайтно), `below_100pips_last` — текст отката шага 5 при −100 пипсов.

| DEX | фабрика | пара | вход | комиссия (пипсы → %) | откат ниже |
| --- | --- | --- | --- | --- | --- |
| dao_swap | 0x940BEb63…6aa4 | 0x73c86eac…a569 | BUSD | 2500 → 0,25 | DAOSwap: K |
| orangedx | 0xfeF9D38f…6E35 | 0x11d75cb0…d947 | USDT | 2500 → 0,25 | Orange: K |
| plearn | 0x4750A965…40D2 | 0x692e3596…4d8c | BUSD | 2000 → 0,20 | Plearn: K |
| pls2e | 0x0944AB69…0415 | 0xc7953c41…17cc (USDT/WBNB) | USDT | 3000 → 0,30 | P2E: K |
| baryon_network | 0x03879E2a…b599 | 0x9f1a332c…bf5c | BUSD | 2500 → 0,25 | Baryon: K |
| omni-exchange-v2-bsc | 0x7d9D5126…7087 | 0xc4c6af4d…41b2 (USDC/WBNB) | USDC | 2500 → 0,25 | OMNI: K |

Факты сверх таблицы:
- Резервы после sync = резерв входа − сумма, резерв выхода тот же — у всех шести.
- Получено владельцем = формула V2 по резервам после sync — у пяти; у dao_swap меньше (монета выхода
  0xa7266989… при переводе из пары шлёт часть на 0x0 и 0x3f5dc910… — налог монеты, `out_token_taxed`).
- omni-exchange-v2-bsc: у фабрики 5 пар, наибольший стейбл-резерв — 1,15 USDC (ликвидности почти нет); комиссия
  измерена на ней.
- pls2e: пара 0x72f7922e…259d (USDT / 0x8267d169…) приняла обмен с комиссией 0 и 2999 — у неё комиссию так не
  измерить (механика монеты не разобрана); у пар WBNB/USDT 0xc7953c41… и 0x958a7344… той же фабрики 2999 — откат,
  3000 — прошло. В запись взята 0xc7953c41….
