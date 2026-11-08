# tests/data/delist_ann_2026-11-08 — время делистинга из анонсов и страницы анонсов (решение 226, уточнение 08.11)

Сняты 08.11.2026 (≈ 14:00–16:30 UTC) в браузере Chrome публичными GET без ключей и без cookies (`fetch(…,
{credentials: 'omit'})` со страницы того же домена; для страниц — заголовок `Accept-Language: en-US`). Без cookies
все четыре сайта отдают английскую страницу, текст анонса — в самом HTML (не только в скриптах).

**Как сжаты страницы:** из полного HTML (35–145 КБ) взят наименьший элемент, содержащий весь текст анонса; у тегов
удалены атрибуты (class, style, href), удалены `script`, `style`, `img`, `svg`, `picture`; пробелы между тегами
сжаты. Текст и порядок не менялись. Проверка полного живого HTML той же логикой (строки с «UTC» без слов про
ботов/депозит/вывод/Convert/Earn и пары `…USDT`) — лишнего нет у KuCoin, Bybit, OKX; у Bitget в блоке «похожие
статьи» есть чужие пары (WIN/USDT, PUMPBTC/USDT, BTW/USDT, EPICUSDT) — записан отдельно (`bitget_page_related_links.html`,
только href и текст ссылок), разбор такие ссылки выбрасывает.

| Файл | Запрос | Что внутри |
|---|---|---|
| `kucoin_page_multiple_tokens_2026-11-05.html` | https://www.kucoin.com/announcement/en-kucoin-will-delist-multiple-tokens-on-2026-11-05 (annUrl из GET /api/v3/announcements) | 23 монеты «Название (ТИКЕР)» (в т. ч. ION — живой случай решения 175), торги до 8:00 05.11.2026 UTC, депозит 03.11, бот 6:00, вывод до 07.12 |
| `bybit_announcements_delistings_all.json` | Bybit GET https://api.bybit.com/v5/announcements/index?locale=en-US&type=delistings (без tag) | 5 из 80 анонсов (4 страницы): фьючерс EPICUSDT (Derivatives), «Bybit to Delist 1 Token(s) on Oct 28, 2026», Bybit Alpha (WEB3), «Bybit to Delist 3 Trading Pair(s) on Sep 24, 2026», «Delisting of L3,VIC». Спотовые — без тега Spot. У url убрана строка запроса (её не было) |
| `bybit_page_hft.html` | https://announcements.bybit.com/en-US/article/bybit-to-delist-1-token-s-on-oct-28-2026--art0d9083316016/ | HFT, пара HFTUSDT, торги до Oct 28, 2026, 8:00AM UTC |
| `bybit_page_swell_pairs.html` | …/bybit-to-delist-3-trading-pair-s-on-sep-24-2026--art93c64e0a0297/ | снимаются только пары к USDC (SWELLUSDC, SANDUSDC, LUNCUSDC), Sep 24, 2026, 8:00AM UTC |
| `okx_page_dora_table.html` | https://www.okx.com/help/okx-to-delist-dora-icx-storj-zeus-and-elf-spot-trading-pairs | таблица «пара | время»: USDT-пары — October 3, 2026, 08:00 - 10:00 UTC, USDC/EUR — September 30 |
| `okx_page_selected_usds.html` | https://www.okx.com/help/okx-to-delist-selected-usds-spot-trading-pairs | 18 пар к USDⓈ, «at 8:00 am UTC on September 18, 2026» (обрезано после этого абзаца — дальше общий текст) |
| `bitget_page_alpine_qubic.html` | https://www.bitget.com/en/support/articles/12560603899692 (annUrl из GET /api/v2/public/annoucements) | ALPINE/USDT; QUBIC/USDT, November 13, 2026, 10:00 AM (UTC) (= offTime справочника), Earn 03:00, Convert 06.11, вывод до 13.02.2027 |
| `bitget_page_related_links.html` | та же страница, блок `a[data-testid=SupportArticleDetailRelatedArticles]` | 10 ссылок на другие анонсы (href и текст, классы и svg удалены) |

Проверено и в файлы не вошло:
- **Phemex** (решение 226, п. 3): 6 пар Listed с timeline[3] в декабре 2025 — GET /md/spot/ticker/24hr: объём 0;
  /md/orderbook: стакан пуст; /md/trade: последняя сделка — в момент timeline[3]. Торги сняты, status устарел.
- **MEXC** GET /api/v3/announcements: 15 страниц × 20 = 19 суток (20.10–08.11) — спотовых делистингов нет, только «MEXC
  Announcement on ST Warning (November 2026)» (= метка st), «Delisting of … from MEXC Convert», фьючерсы, «Tokens
  Moved to the Assessment Zone».
- **KuCoin** без pageSize — 15 на странице; за 30 суток 4 анонса delistings, за 90 — 19 (Earn — «KuCoin Earn Will
  Delist …»). **Bybit** — 11 анонсов за 30 суток. **OKX** — последний спотовый делистинг 23.09. **Bitget** — 4 за месяц.

Кто читает: `tests/delist_ann.rs`.
