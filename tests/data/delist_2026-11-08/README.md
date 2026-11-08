# tests/data/delist_2026-11-08 — живые ответы бирж для решения 226 (источники делистинга)

Сняты 08.11.2026 (≈ 11:30–12:45 UTC) в браузере Chrome публичными GET-запросами без ключей: страница домена API
биржи + `fetch` того же домена из консоли. Ответы **сжаты отбором**: из полного ответа взяты только нужные записи
(характерные и пограничные), у записей оставлены поля, которые читает разбор; значения не менялись. Сводка по полному
ответу — в скобках.

| Файл | Запрос | Что внутри |
|---|---|---|
| `mexc_exchange_info_st.json` | MEXC GET https://api.mexc.com/api/v3/exchangeInfo | NTXUSDT, ASSUSDT (st true), BTCUSDT (st false), DOPEUSD1 (st true, котируемая USD1). (1800 пар, все status "1", tradeSideType 1; st true у 49: 46 к USDT, 3 к USD1) |
| `kucoin_symbols_st.json` | KuCoin GET https://api.kucoin.com/api/v2/symbols | BTC-USDT (st false), POLS-USDT, SOUL-USDT, P2P-USDT (st true). (955 пар, st true у 22; BLAST, VENOM, NFP, ION уже сняты) |
| `kucoin_announcements_delistings.json` | KuCoin GET /api/v3/announcements?annType=delistings&lang=en_US&pageSize=50 | полный ответ: 4 анонса за месяц (по умолчанию биржи) |
| `bybit_instruments_spot_st.json` | Bybit GET https://api.bybit.com/v5/market/instruments-info?category=spot&limit=1000 | BTCUSDT, FTTUSDT, CTCUSDT, CHZUSDC, DYDXUSDT (stTag "0"), DYDXUSDC (stTag "1"). (521 пара, все Trading, stTag "1" у 13) |
| `bybit_announcements_delistings.json` | Bybit GET /v5/announcements/index?locale=en-US&type=delistings&limit=50 | 5 анонсов: залог, фьючерс EPICUSDT, VANRY (Spot), «Spot Token Delisting Notice», «Delisting 11 Spot trading pairs…». (total 493) |
| `bybit_announcements_spot.json` | Bybit GET /v5/announcements/index?locale=en-US&type=delistings&tag=Spot&limit=50 | 5 анонсов: VANRY, SOLO, ELX/ODOS/DMAIL, BTC/BRZ и USDT/BRZ, leveraged tokens (у двух описание обрезано до 160 знаков, у двух — без description/start/end). (total 59) |
| `okx_instruments_spot.json` | OKX GET https://www.okx.com/api/v5/public/instruments?instType=SPOT | BTC-USDT. (1210 инструментов, все live, expTime и upcChg пусты у всех) |
| `okx_announcements_delistings.json` | OKX GET /api/v5/support/announcements?annType=announcements-delistings, заголовок Accept-Language: en-US, без cookies | 11 из 20 анонсов страницы 1 (спотовые, фьючерсные, миграции, «postpone»); totalPage 7. Без Accept-Language браузер получал русские заголовки |
| `bitget_spot_symbols.json` | Bitget GET https://api.bitget.com/api/v2/spot/public/symbols | BTCUSDT, ALPINEUSDT и QUBICUSDT (offTime 1794564000000 = 13.11.2026 10:00 UTC), WEETHUSDT (halt). (3668 пар, offTime у 2) |
| `bitget_announcements_symbol_delisting.json` | Bitget GET /api/v2/public/annoucements?language=en_US&annType=symbol_delisting | первый анонс: «Notice of Delisting 2 Spot Trading Pairs on November 13, 2026» — сверка смысла offTime (тикеров в заголовке нет) |
| `phemex_products_plus.json` | Phemex GET https://api.phemex.com/public/products-plus | спот: BTCUSDT, BTCUSDC, 6 Listed с timeline[3] > 0 (декабрь 2025), SUSHI и GRASS (stStatus 1), 2 Delisted. (1019 спот-пар: 176 Listed, 843 Delisted; у Listed timeline[3] в будущем нет ни у одной) |
| `htx_v1_common_symbols.json` | HTX GET https://api.huobi.pro/v1/settings/common/symbols | c98usdt, grassusdt (online, dl false, tca 2030), 4 suspend с dl true, altusdt (online, dl false, tca 2029), ckbht (offline). (2067 пар) |

Проверено и в файлы не вошло (нет записей для теста): BingX common/symbols — status 29 нет ни у одной пары, offTime
будущих нет (1265 в прошлом); XT GET /v4/public/symbol — nextState / nextStateTime null у всех 1110 пар; MEXC
GET /api/v3/symbol/offline — только снятые (state 3); MEXC GET /api/v3/announcements — без тикеров в спотовых
заголовках.

Кто читает: `tests/delist_sources.rs`.
