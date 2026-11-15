# fit_volume_2026-11-15 — запись цепи для решения 294

Что: состояние пула PancakeSwap V3 SHOPINX/WBNB и пары PancakeSwap V2 WBNB/USDT на одном блоке — для тестов
уменьшения объёма до помещающегося (tests/core_fit_volume_294.rs).

Как записано: 15.11.2026, вкладка браузера на https://bsc-rpc.publicnode.com, только `eth_call` (и `eth_blockNumber`)
одним блоком **133732042 (0x7f896ca)**, JSON-RPC batch; формат `responses.json` — как у рекордера
tests/recorders/record_route.js (поля `results[].name / request / response`), читается tests/route_common `load`.
Ответы перенесены из вкладки как есть (hex результата), запросы восстановлены по тем же адресам, селекторам и блоку.

| Пул | Адрес | Вид | Вызовы |
| --- | --- | --- | --- |
| SHOPINX/WBNB | 0x3079ae2e87dc2e0c75c9b98d65bc622ad10f326c | PancakeSwap V3 | tickSpacing, slot0, liquidity, token0, token1, fee; TickLens `getPopulatedTicksInWord` (0x9a489505a00ce272eaa5e07dba6491314cae3796 — тот же, что в routes_2026-10-30/calls.json) по словам −447…−423 |
| WBNB/USDT | 0x16b9a82891338f9ba80e2d6970fdda79d1eb0dae | PancakeSwap V2 | getReserves, token0, token1 |
| токены | 0x4f0e…0d6b (SHOPINX), 0xbb4c…095c (WBNB) | ERC-20 | decimals |

Факты записи:
- token0 пула = 0x4f0ed1f48beaa4797a62202a21804cadc2170d6b (symbol «SHOPINX», decimals 18), token1 = WBNB
  0xbb4cdb9cbd36b01bd1cbaebf2de08d9173bc095c (symbol «WBNB», decimals 18); fee = 100, tickSpacing = 1.
- slot0: tick = −111229 (цена ≈ 1,4779·10⁻⁵ WBNB за SHOPINX), liquidity = 0xbd18a116fa80fdbb2b (≈ 3488,2·10¹⁸).
- Все 25 слов TickLens (±12 вокруг слова текущего тика −435) — пустые массивы. Сверка той же вкладкой на том же блоке
  `tickBitmap(int16)` пула (ответы в файл не перенесены — только итог): слова −435, −500, −400 — 0; слова −3466 и 3465 —
  по одному установленному биту. Позиции пула — только на краях диапазона тиков, внутри прочитанной карты ликвидность
  постоянна. Сколько слов читает бот (dex.tick_words) на сервере, здесь не установлено; тест берёт ±3.
- WBNB/USDT: token0 = USDT 0x55d398326f99059ff775485246999027b3197955, token1 = WBNB; резервы
  ≈ 17 479 285 USDT / 29 296,8 WBNB.
- symbol записан только в расшифрованном виде (hex ответа не перенесён) — тест его не читает.
