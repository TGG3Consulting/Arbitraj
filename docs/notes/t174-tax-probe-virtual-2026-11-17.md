# t174 — проба налогов у DEX с виртуальным квотером (решение 388), 17.11.2026

## Что было

Факт живого бота (из задания): журнал Error `dex.check_not_configured` «Проверка налогов: не задана настройка —
metric-bsc: Metric сверяется только виртуальным квотером — задайте dex.quoter_virtual = да», хотя у metric-bsc
`dex.quoter_virtual` = да.

Путь по коду (ветка rel/dex-all, a6ad7802):

- `check/taxes.rs:182` → `check/probe.rs run`. Сначала собирается своп пробы (`part`, строки 103–115): у всех пяти видов
  `family.tradable = false` (`pools/families.rs:333, 349, 351, 352, 356`) и есть шаг исполнителя
  (`pools/exec_kind.rs`: algebra_integral → ALGEBRA, metric_omm → METRIC, iziswap / biswap_v3 → IZI, smardex → SMARDEX),
  поэтому своп идёт через `check/probe_exec.rs part` и (при развёрнутом исполнителе) проходит.
- Затем вызов «ожидалось без налога»: `probe.rs:116` и `:172` → `quote/v2_pair_probe.rs request_any` (:58–69) → у
  не-V2-пары сразу `quote/quoter_calls.rs request`. Там (:56–84) для `Proto::AlgebraIntegral`, `Proto::MetricOmm`,
  `is_izi()` (IziSwap, BiswapV3) и `Proto::Smardex` всегда `ctx.not_set("dex.quoter_virtual", …)`. `virtual_quoter::on`
  на этом пути нигде не проверялся (в отличие от `quote/quoter.rs:53 out_at`).
- `taxes.rs`: `NotConfigured` → `config` («не задана настройка»), налоги пула не записываются (`f.taxes` остаётся None).
- `quote/pool_tax.rs:29–38 usable`: налог стороны не измерен пробой → пул по стороне не используется.

Итог: у пулов Metric OMM, Algebra Integral, iZiSwap, Biswap V3, SmarDex налоги пробой не измерялись никогда, такие пулы
не котировались и не торговались (с решений 188, 285, 286, 288). Подтверждено по коду для каждого из пяти видов.

Пример на пальцах: пул NVDAB/USDT Metric (0x7201…d117). Проба: USDT на кошелёк → разрешения → вызов «ожидалось» →
`quoter_calls::request` → Err «не задана настройка» → проба не запускается → налог покупки/продажи пула = «не измерен» →
`usable` отвечает «налог покупки монеты в этом пуле не измерен пробным обменом» → пул выпадает из котировки и графа.

## Что сделано

- `quote/probe_quote.rs` (новый): `Way { virt }`; `Way::of` = `virtual_quoter::on` (один раз на пробу — вызов и разбор
  одного квотера). `request` — при `virt` адрес `virtual_quoter::address()` и данные `virtual_quoter::request`
  (quoteExactIn / quoteMetric / quoteIzi / quoteSmardex), иначе `v2_pair_probe::request_any` (как было). `overrides` —
  код квотера по его адресу (сверка хеша `runtime()`). `decode` / `expected` — `virtual_quoter::decode`; «взял меньше
  входа» (SHORT) при покупке — NoRoute (помеха, как откат QuoterV2 «не хватает ликвидности» на шаге квотера в
  `probe_calls::buy_revert`).
- `check/probe.rs`: обе симуляции — `simulate::run_with(…, &over, at)` (как `exec/dry_surplus_tail.rs`, решение 186);
  вызовы и разбор «ожидалось» — через `Way`.
- `check/probe_calls.rs sell_result`: принимает `Way`; SHORT при продаже — налог продажи не измерен (`SellMiss`,
  reverted = false — помеха), прочее — как раньше.
- `quote/quoter_calls.rs`: четыре ветки сведены в `virtual_only` + `virtual_missing`. Флаг не «да» — прежний текст и
  `NotConfigured` с именем настройки. Флаг «да» — `Internal` «вызов прошёл мимо виртуального квотера (ошибка кода)», без
  совета задать уже заданное.
- Глобально: у DEX вида V3 / Algebra (THENA Fusion) / Slipstream с `dex.quoter_virtual` = да проба тоже идёт
  виртуальным квотером (раньше — QuoterV2 из `dex.quoter`), как и сверка `quote/quoter.rs out_at`.
- Новых настроек нет. Документы: DECISIONS 388, ARCHITECTURE, README `quote/`, `check/`, `tests/`, TASKS.

## Что доказывают тесты (`tests/bsc_dex_probe_quote_t174.rs`) и что нет

На записи `tests/data/metric_2026-11-15` (блок 133656733, пул NVDAB/USDT):
- с флагом — вызов пробы по адресу виртуального квотера, селектор quoteMetric 0x9229d10b, флаг zeroForOne для
  «USDT → монета» = записанному; подмена кода — код квотера по его адресу;
- записанный ответ (used, out) разбирается в «ожидалось» = out (покупка и `sell_result`).

Для всех пяти видов (вид подставлен в ту же запись пула, чистая логика): без флага — прежнее `NotConfigured`
«задайте dex.quoter_virtual» и в квотере DEX, и в пробе; с флагом — проба по адресу виртуального квотера с данными
`virtual_quoter` своего вида, квотер DEX — `Internal` без «задайте».

Чистая логика (помечено в тесте): сумма больше записанного used → покупка NoRoute, продажа — «не измерен» (помеха);
балансы продажи — числа теста.

НЕ доказано: сама eth_simulateV1 пробы с подменой кода на живом узле (записи нет) — что пул Metric / SmarDex / iZi /
Integral в симуляции от кошелька пробы отдаёт тот же выход, что в eth_call, и что налог считается верно. Запись запрошена
(docs/TASKS.md). Подмена кода в eth_simulateV1 уже используется хвостом сухой сделки (решение 252, вариант «б»).

## Что запросить с сервера (только чтение)

1. После выкладки: журнал `dex.check_not_configured` за первые часы — нет строк «сверяется только виртуальным
   квотером» по DEX с `dex.quoter_virtual` = да (metric-bsc и др.).
2. Журнал проверки налогов (строки «не проверено» / «не прошла» по монетам с пулами этих DEX) — причины по пулам
   этих видов: появились ли «квотер пробной покупки — виртуальный квотер: пул взял не весь вход», `Internal`
   «прошёл мимо виртуального квотера», откаты шага квотера.
3. Факты пулов (`PoolFacts.taxes`, экран «Монеты» → пул) по 2–3 пулам Metric и SmarDex — налоги покупки/продажи
   измерены.
4. Для теста на записи: сырые запрос и ответ обеих eth_simulateV1 пробы налогов (сырой журнал узла) по одному пулу
   каждого вида — в tests/data.

## Ревью (правки поверх 2291353d)

- **Н1.** Виртуальный квотер пробной покупки откатился на своём шаге (причина пула на этом блоке: цена оракула Metric
  устарела, пауза, «VQ: no callback»), а покупка в той же симуляции прошла. Раньше `probe_calls::buy_revert` считал это
  вердиктом по монете: `quoter_revert::classify` такой откат не распознаёт. Теперь это помеха:
  `probe_quote::Way::quote_reverted` → NoRoute → «не проверено», повтор. Если покупка тоже откатилась или квотер не
  виртуальный — как раньше.
  Факт кода о чёрном списке: Failed → `BlockReason::CheckFailed` (`core/blacklist.rs:103`), но не насовсем:
  - полный проход раз в `tax.recheck_interval_s` проверяет все монеты (`check/tax_cycle.rs`, `tax_pass::check` по
    всему списку);
  - итог Known заменяет Failed (`tax_pass.rs:155–160`);
  - помеха (Wait) прежний Failed не меняет (`tax_pass.rs:172–175`).
  
  То есть Failed держался до прохода с удачной пробой или до «Перепроверить».
- **Н3 (охват, факт кода).** У DEX вида V3-форк / PancakeV3 / UniswapV3 / Algebra (THENA Fusion) / Slipstream с
  `dex.quoter_virtual` = да `dex.quoter` не обязателен (`pools/families.rs:633–636` убирает его из required, список —
  519–549). Проба до 388 шла в `quote/quoter_calls.rs` `ctx.dex_addr("dex.quoter")` (строки по 2291353d: :240 Algebra, :275 Slipstream, :352 V3) →
  «не задана настройка». У таких DEX без `dex.quoter` налоги тоже не мерились никогда, теперь мерятся: котировка и
  торговля через их пулы откроются шире пяти видов. Какие именно DEX — по настройкам сервера (запрос ниже). У DEX
  этих видов с флагом и с заданным `dex.quoter` проба тоже переходит с QuoterV2 на виртуальный квотер.
- `virtual_only` — через `Proto::is_izi` (ровно IziSwap и BiswapV3, `families.rs:232–234`).
- Тест Н1 — `virtual_quote_revert_with_buy_ok_is_interference`. Это чистая логика: записан только селектор отката
  PriceProviderFailed пула QQQB, сочетание «квотер откатился, покупка прошла» не записано.

**Запросить с сервера дополнительно (только чтение):**
- список DEX с `dex.quoter_virtual` = да и без `dex.quoter`: у них налоги начнут мериться впервые;
- после выкладки — строки «виртуальный квотер пробной покупки откатился … покупка прошла» в причинах проверки налогов:
  сколько и по каким пулам.
