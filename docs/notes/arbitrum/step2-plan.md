# Две сети, шаг 2: ядро «много DEX» — инвентарь по коду и план (решение 430, 19.11.2026)

Основа: ветка `feat/arbitrum`, HEAD `c2d1d47b` (шаг 1 = решение 416 сделан). План всего пути —
`docs/notes/arbitrum/multichain-plan-2026-11-19.md`. Главное условие шага: **BSC после шага работает точно как раньше**,
все существующие тесты — без изменения смысла.

Источник: только факт кода (grep по `src/`, номера строк — на `c2d1d47b`). cargo локально не запускался — проверка в CI.

## 1. Главное решение шага: «сеть по умолчанию» = `None`, переноса данных BSC нет

Факт кода: после шага 1 у всех монет `Asset.chain = None` (литералы `chain: None` во всём `src/` и `tests/`), записи
учёта без поля «chain» читаются с `None`; равенство и хэш монеты — (сеть, адрес) структурно (`core/types/market.rs:46-77`).
Если ядро начнёт ставить `Some(bsc)` монетам BSC — старая монета (`None`) ≠ новой (`Some(bsc)`): ломаются ключи
замков, идущие сделки, карантин, окна, учёт и **все тесты**, где монета собрана с `chain: None` (их сотни).

Поэтому в шаге 2 принят **канонический вид** монеты:
- **сеть по умолчанию** — одна из сетей площадок DEX: если у всех площадок DEX одна сеть (`dex.chain`) — она (как в
  шаге 1, `core/chain.rs dex_chain`); если сетей несколько — новая настройка вся система `dex.default_chain`
  (id сети, монеты которой пишутся без сети; для живого бота — `bsc`); не задана при двух сетях — ошибка с причиной
  (как в шаге 1 «сеть не выбрать»), а не догадка;
- монета сети по умолчанию — всегда `chain: None` (запись, ключ, JSON — байт в байт как до 416);
- монета другой сети — `chain: Some(id)`. Ставит её только ядро через `core/chain.rs tag` (одна функция).

Итог: живая база BSC (core_state, учёт сделок, окна, настройки монет `0x…`, пары `bingx:cex_dex`) читается без
переноса и без ручных правок; переносить нечего — у BSC ключи не меняются. Новые ключи появляются только у монет и
направлений второй сети (с суффиксом сети). Это и есть «перенос ключей» шага: правило «нет сети = сеть по умолчанию»
вместо переписывания базы (как решение 211 сделало запасное чтение по старому ключу).

## 2. Инвентарь: где ядро/шлюз/отчёты/API предполагают одну DEX или одну сеть

### 2.1 «Первая DEX» (`Core::dex_id`) и зашитый id `bsc_dex`
| Место | Что предполагает |
|---|---|
| src/core/balances_cache.rs:181-187 | `dex_id()` — первая площадка вида Dex в реестре |
| src/core/balances_cache.rs:125-178 | `wallet_balances(wallet)` — балансы кошелька на «первой DEX»; кэш по id кошелька без сети (:31) |
| src/core/usdt_sources.rs:74 | источники USDT «Перегона USDT» — кошельки только на первой DEX |
| src/core/drawdown_fact.rs:49 | площадка покупки пары DEX→биржа / DEX↔DEX — первая DEX |
| src/core/day_pair.rs:111 | площадка пары «DEX ↔ DEX» для лимита дня — первая DEX |
| src/core/deal_card.rs:25 | `const DEX = "bsc_dex"` — у какой площадки показывать кошелёк в пути сделки |
| src/core/start_note.rs:91 | тест: литерал `bsc_dex` (только тест, поведение не задевает) |
| src/report/cap_send.rs:45, report/snap_dex.rs:69, report/snap_transit.rs:61 | отчёты: адрес/баланс кошелька — на первой DEX |
| src/report/cap_limits.rs:147 | отчёт лимитов: «DEX ↔ DEX» — первая DEX |
| src/api/wallets_dex.rs:41-60,69 | API кошельков: первая DEX (баланс, исполнитель, проверка) |
| src/api/wallets.rs:66, src/api/wallets_view.rs:41,53 | то же через `wallets_dex::dex_id` |
| src/notify/ctl/view.rs:59 | баланс кошелька в Telegram — через `wallet_balances` (первая DEX) |

### 2.2 Газ, USDT, нативная монета — «одна на всё ядро»
| Место | Что предполагает |
|---|---|
| src/core/gas_ticker.rs:40-59 `of_core` | один тикер газа на все DEX (разные → ошибка) |
| src/core/gas_ticker.rs:63-71 `for_venue` | у биржи — тикер «сети площадок DEX» (`of_core`) |
| src/core/subscribe.rs:73 | бирже — стакан одного газа (`of_core`) |
| src/core/convert_at.rs:42 | «это газ?» — по одному тикеру (`of_core`) |
| src/core/gas_stats.rs:104 | цена газа впустую — по одному тикеру (`of_core`) |
| src/core/deal_gas.rs:104 | газ вне обмена — `for_venue` (у биржи сделки — `of_core`) |
| src/core/inner.rs:51-52 `usdt` | один адрес USDT «в BSC» на всё ядро |
| src/core/registry_build.rs:47-67 `usdt_address` | USDT — из справочника сетей бирж (сеть BSC) |
| src/core/rebuild.rs:164,171 | хранение одного USDT |
| src/core/subscribe.rs:213, transfer_tax.rs:58, credit_shortfall.rs:46, usdt_return.rs:49, coins_view.rs:39,58, dirctx.rs:177, venue_ticker.rs:79, mid_wake.rs:142 | читатели одного USDT |
| src/core/native.rs:36-57 `read` | одна нативная монета на все DEX (BNB ≠ ETH → Err → сопоставление BNB↔WBNB пропадает у BSC) |
| src/core/native.rs:84, 150-176 (`map`, `ticker`, `is_native`, `current`) | по всем DEX сразу |
| src/core/rebuild_build.rs:52-58 | нативная — по всем DEX |

### 2.3 Справочник, presence, подписки
| Место | Что предполагает |
|---|---|
| src/core/registry_build.rs:127-152 | DEX-присутствие монеты ставится КАЖДОЙ площадке DEX (без сети) |
| src/core/registry_build.rs:139-143 | монета справочника всегда `chain: None` |
| src/core/presence.rs:27-60 `cex` | сеть биржи — одна (`venue.bsc_network`) |
| src/core/subscribe.rs:46-58 | каждой DEX — все монеты справочника |
| src/core/subscribe.rs:59-68 | каждой DEX — USDT одной сети и промежуточные ВСЕХ DEX (`intermediate_assets(core, None)`) |
| src/core/subscribe.rs:83-89 | нативная — `native::ticker(core)` по всем DEX |
| src/core/transfer_tax.rs:30-48 `measured` | налог перевода — максимум по ВСЕМ DEX (без сети монеты) |
| src/core/coin_account.rs:106-110 | сеть по умолчанию — `dex_chain` (две сети → нет) |

### 2.4 Сеть перевода
| Место | Что предполагает |
|---|---|
| src/core/transfers.rs:55-107 `network`, `own_bsc_name` | имя сети — `venue.bsc_network` площадки, одна сеть |
| src/core/dirctx.rs:155-169 | сеть сторон направления — `venue.bsc_network` |
| src/core/rebuild.rs:45,97 | `venue.bsc_network` каждой биржи — одна сеть |
| src/core/usdt_transfer_plan.rs (шапка :13) | перегон USDT — `venue.bsc_network` |
| src/gateway/network_hint.rs, src/settings/hint.rs:11, api/check.rs:90 | подсказка имени сети — только BSC (шаг 3) |

### 2.5 Ключи настроек и учёта без сети
| Место | Что предполагает |
|---|---|
| src/settings/venue_dir.rs:79-100 `id`/`parse_id` | пара «bingx:cex_dex», «dex_dex» — без сети: BingX→DEX BSC и BingX→DEX Arbitrum сольются |
| src/core/day_pair.rs:30-110 | пара итога дня/просадки — без сети |
| src/settings/scope.rs:88-100 `coin_id`/`is_address_id` | id области «монета» — только «0x…» без сети |
| src/core/quarantine.rs `key_of_id`, src/core/deposit_hold.rs | ключ из интерфейса / удержание — без сети (вопрос шага 1, TASKS) |

### 2.6 Газ в именах `*_bnb` (косметика типов и базы)
57 мест `*_bnb` в src/core, api, report, gateway; 16 — `risk.min_bnb_balance` вне адаптера. Имена в JSON и базе
менять нельзя (живой учёт), в коде — только переименование.

### 2.7 Перебор «всех DEX» — уже по площадке (менять не нужно, проверено)
cycle_watch.rs:48 (циклы DEX↔DEX — по каждой DEX своё), signal_ctx.rs:37, precheck*.rs (вид площадки сделки),
market_state (ключи `(площадка, адрес)` — площадка DEX = одна сеть, сеть уже разделена), card.rs:122 (DEX сделки).

## 3. План правок (коммитами по частям)

**2.1 Модель сетей ядра** (`core/chain.rs`, настройка `dex.default_chain`):
`default_chain` (одна сеть у DEX — она; иначе `dex.default_chain`), `tag` (сеть по умолчанию → None), `dex_of`
(площадка DEX сети; две площадки на одну сеть — ошибка), `chain_of_dex`, `of_dir`. `Core::dex_id()` удалить —
вместо него `chain::default_dex` (площадка DEX сети по умолчанию, детерминированно) и перебор всех DEX там, где это
список (кошельки «Перегона USDT», отчёты). `deal_card.rs` — вид площадки из реестра, а не id «bsc_dex».
Кэш балансов кошелька — по (площадка DEX, кошелёк). Одна DEX — всё как раньше.

**2.2 Газ по сети**: `gas_ticker::of_chain`, `of_asset` (сеть монеты → площадка DEX → `dex.gas_ticker`), `all`
(тикеры газа всех DEX — бирже стаканы всех). `of_core` убрать из ядра; `for_venue` у биржи — по сети монеты сделки.

**2.3 USDT, нативная, справочник, подписки по сети**: USDT — карта «сеть → адрес» (сеть по умолчанию — как раньше из
справочника бирж; другие сети — когда биржи отдадут адрес по сети, шаг 3) с прежним `inner.usdt` = сеть по умолчанию;
`native::read` — по площадкам DEX одной сети; справочник — DEX-присутствие только у DEX сети монеты; подписка DEX —
монеты своей сети, свой USDT, свои промежуточные; налог перевода — по DEX сети монеты.

**2.4 Сеть перевода по направлению**: сеть направления = сеть монеты (`chain::of_dir`). Сеть по умолчанию — имя
`venue.bsc_network` (как раньше); другая сеть — имя сети из справочника биржи по этой сети (шаг 3); у DEX — своя
`venue.bsc_network` площадки DEX. Нет имени — перевода нет, причина «имя сети … у биржи не задано (шаг 3)».

**2.5 Ключи второй сети**: пара `venue_dir` — у сети не по умолчанию суффикс «@сеть» («bingx:cex_dex@arbitrum»,
«dex_dex@arbitrum»); id области «монета» — «сеть:0x…». У сети по умолчанию ключи прежние.

**2.6 `*_bnb` → `*_native`** — переименование в коде с serde-именами прежними. Отложено, если не хватит шага: на
поведение не влияет (ROADMAP, техдолг).

Не в этом шаге: адаптер Arbitrum (шаг 5), биржи по сетям — адрес и имя сети по каждой сети (шаг 3), кнопки и
кошельки по сети в UI (шаг 4). Шаг 2 им не мешает: всё по сети берётся из `core/chain.rs`.

## 4. Проверка «BSC как раньше»

Одна площадка DEX (`bsc_dex`, `dex.chain = bsc`): `default_chain` = bsc, `tag(bsc)` = None, `dex_of(None)` = bsc_dex,
`gas_ticker::of_asset` = `dex.gas_ticker` bsc_dex, USDT сети по умолчанию = прежний, DEX-присутствие и подписка —
те же монеты (все с `None`), сеть перевода — `venue.bsc_network`, ключи пар и монет — прежние. Новая настройка
`dex.default_chain` при одной сети не читается.

## 5. Сделано (19.11.2026)

- a40468c2 — 2.1 сети ядра: core/chain.rs (dex.default_chain, tag, dex_of, default_dex, tag_of_dex, of_dir), Core::dex_id
  удалён, балансы кошелька по (DEX, кошелёк), «Перегон USDT» по всем DEX, отчёты/API — DEX сети по умолчанию.
- 405067aa — 2.2 газ по сети (gas_ticker of_chain / of_asset / all / for_venue_in).
- ef0cebfe — 2.3 справочник по сетям (core/registry_chains.rs), USDT других сетей (Inner.usdt_other), подписка DEX,
  нативная монета, налог перевода (chain::kinds_in), USDT по сети в ядре.
- 8cb6a00e — 2.4 сеть перевода по направлению (transfers::network по монете, own_name).
- Не сделано (2.5 ключи второй сети, 2.6 *_bnb, отображение, карты по адресу) — docs/TASKS.md, блок 430.

