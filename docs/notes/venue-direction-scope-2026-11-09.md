# Настройки денег и прибыли по бирже и направлению — анализ (решение 242, 09.11.2026)

Задача Тиграна (09.11, согласовано): money.min_profit_usd, money.min_profit_pct, money.max_per_trade_usd,
profit.price_move_k, profit.closure_share — по паре «площадка + тип направления» с откатом на прежнее значение.
Всё ниже — факт кода ветки feat/own-bsc-node (после «Слияние t58», 13135e9).

## 1. Как устроены области

- **Каталог** (`src/settings/catalog/*.rs`, `schema.rs`): `SettingDef.scopes: &[ScopeKind]`; виды — Global, Venue, Coin,
  Event, Dex, Channel. Наборы в `catalog/mod.rs`: G, GV, GC, V, VC, C, E, DX, CH. Смысл площадки — `venue_role`
  (Venue / TransferSource / TransferTarget). Вида «тип направления» не было (это и есть вопрос 2 решения 240).
- **Хранилище** (`store.rs`): таблица `settings(name, scope, value, updated_at)`, `scope` — текст `Scope::to_key()`
  («global», «venue:bingx», «coin:0x…», «event:…», «dex:…», «channel:…»); история — `settings_history` (тот же ключ).
  Загрузка: `Scope::from_key`, нераспознанная строка пропускается с записью в лог. Кэш — `HashMap<(name, Scope), Value>`.
  Новый вид области = новый префикс ключа; переноса данных не нужно (старые ключи не меняются).
- **Чтение**: `Store::resolve(name, venue, coin)` — монета → площадка → вся система; `read.rs` (`decimal(name, At)` и др.);
  `read_scope.rs` — ровно в области.
- **Проверка** (`value.rs validate`): вид области должен быть в `def.scopes`; `check_scope_id` — id не пустой и т. п.
- **API** (`api/settings.rs`): схема (каталог целиком, serde), значения `{name, scope:{kind,id}, value}`, PUT, история.
- **Экспорт/импорт** (`api/settings_io.rs`): запись = имя + `Scope::to_key()` + значение; импорт — `Scope::from_key` и
  `Store::set` (та же проверка и журнал).
- **Экран**: `ui/src/pages/Settings.svelte` (категории = `group`), `SettingCard.svelte` (строки по областям),
  `SettingEditor.svelte` (поле, «Сохранить», «Очистить»), `SettingHistory.svelte` (история настройки).
- **FIRST_RUN**: `config/first_run.toml` (+ зеркало docs/FIRST_RUN.md), предзаполнение по `{kind, id}`.

## 2. Где читаются пять настроек (до правки)

| Место | Настройка | Область чтения | Direction |
| --- | --- | --- | --- |
| core/profit.rs decide_capped (кандидат, предпроверка, refit) | min_profit_usd/pct | вся система | есть (d) |
| core/card_max.rs compute (карточка сигнала) | min_profit_usd/pct | вся система | calc.direction |
| core/uncapped.rs compute («без потолка», «Упущено»; сделка — копия настроек 239) | min_profit_usd/pct | вся система | есть |
| core/uncapped_recompute_cfg.rs Cfg::read (пересчёт старых записей по окну, бинарь uncapped_recompute) | min_profit_usd/pct | вся система | у вызывающего prepare (ядра нет) |
| core/precheck_dex_sell.rs check (симуляция продажи до покупки) | min_profit_usd/pct | вся система | есть |
| core/prices.rs for_direction (сигнал тика, потолок объёма сравнения; окна — window_dex) | max_per_trade_usd | площадка A → система | есть |
| core/limits_parts.rs parts (бюджет сделки, «без потолка» — Battle.parts) | max_per_trade_usd | площадка A → система | есть |
| core/cycle_plan.rs limits (цикл DEX ↔ DEX) | max_per_trade_usd | площадка DEX → система | `direction(c)` (buy = sell = DEX) |
| core/dirctx.rs build_with (план, held, decided — копия 239) | price_move_k | монета → система | есть |
| core/dirctx.rs → reserve::closure_share | closure_share | площадка B | есть |
| core/uncapped_recompute_ctx.rs prepare | price_move_k, closure_share | монета → система; B | есть, ядра нет |
| core/risk_ready.rs settings_ready (торговля включается?) | все обязательные money.* | resolve(buy_on) | есть |

Окна (window.rs), «Упущено» сделки (deal_ceiling.rs), refit.rs, отчёты (report/) пороги сами не читают — берут вердикт
profit.rs / запись uncapped.rs. Исполнитель (exec) и отчёты этих настроек не читают. Цикл DEX ↔ DEX из пяти читает
только money.max_per_trade_usd: его порог — dex_dex.min_profit_usd [площадка DEX], запаса и перевода нет.

Вид площадки: `core.inner.kinds` (VenueKind Cex/Dex из шлюза). В пересчёте по окну ядра нет — вид сторон берётся из
плана, как уже делает prepare: покупка на DEX не пересчитывается, продажа на DEX — `plan.sell_quote.route`.

## 3. Расчёты «без потолка», «Упущено», окна, предпроверки

Читают пороги так же, как решение о сделке, и после правки берут их той же функцией (`core/venue_dir.rs thresholds`) с
той же парой; сделка «без потолка» — из копии настроек момента входа (решение 239), как раньше.

## 4. Что меняется

- Новый вид области `ScopeKind::VenueDir` / `Scope::VenueDir(id)`: id «площадка:тип» (cex_dex, dex_cex, cex_cex) или
  «dex_dex» (один набор на все циклы — уточнение Тиграна 09.11), ключ «venue_dir:…». Типы у настройки — `dir_kinds`.
- Чтение: `Store::decimal_dir(name, пара, At)` / `resolve_dir` — значение пары, иначе ровно прежнее `decimal` / `resolve`.
- Правило стороны — `core/venue_dir.rs rule/of`; все места из таблицы переведены на него.
- Пустые значения пар → те же функции с теми же аргументами → поведение прежнее (тесты tests/settings_venue_dir.rs,
  tests/core_venue_dir.rs и прежние тесты расчёта).

## 5. Открытые вопросы

1. profit.price_move_k: пара «площадка + направление» сильнее значения монеты (по постановке: «не задано для пары —
   прежняя цепочка»). Если монета должна быть сильнее пары — поменять порядок в Store::decimal_dir для этой настройки.
2. **Сделано (Тигран 09.11: согласовано; DECISIONS «242 (дополнение, п. 1)», settings/venue_kinds.rs):** проверка при
   записи по видам площадок шлюза. Было: вид площадки в области не проверяется хранилищем (оно не знает видов шлюза): «mexc:cex_dex» для DEX-площадки можно
   записать через API/импорт, но правило стороны его никогда не прочитает. Экран предлагает только верные пары.
