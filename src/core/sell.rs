//! Файл: core/sell.rs
//! Что делает: продажа монет сделки там, где они сейчас (шаг 5 ТЗ). Выгодно — итог сделки при продаже
//! сейчас не ниже нуля (решение 47: любая прибыль забирается сразу; порог прибыли решает только вход) —
//! продаём; иначе — режим застрявшей монеты (stuck.rs, решение 37).
//! Продажа по рынку — для режимов «Ожидание», «Порог убытка», «Перегон» и кнопки «Продать сейчас».
//! Продано всё — сделка закрыта (возврат USDT — общий шаг закрытия ledger::finish, решение 138); продано не всё —
//! остаток снова через режим застрявшей монеты (меньше шага или минимума ордера биржи — пыль, dust.rs, решение 66); продажа не прошла — алерт человеку
//! (manual.rs, решение 100), монеты на месте; продажа не ушла в сеть (решение 110; ни один канал не принял, и узел
//! подтвердил, что транзакции нет, — решение 154) — повтор через режим застрявшей монеты, не больше
//! stuck.unsent_retries_max раз (sell_unsent.rs); решение 229 — так же продажа DEX, не попавшая в блок, когда nonce занят
//! нашей отменой в блоке, и отказ до отправки после кнопки человека; исход продажи неизвестен — сделка остановлена до ручной проверки.
//! Решение 148: сделка закрыта продажей — сверка средней цены продажи с планом (sell_slip.rs).
//! Решение 177: продажа кнопкой человека — итог отдельным событием (manual_outcome.rs); остаток после частичной продажи
//! и застрявшая BNB уходят в режим застрявшей монеты уже как автоматика (risk_count::auto).
//! Решение 227: котировка продажи (средняя, исполнимо, возраст) и худшая цена ордера со стаканом (возраст) — строкой в
//! хронологию сделки; худшая цена — только по свежему стакану (worst.rs worst_order). Решение 238: котировка и худшая
//! цена биржи — один стакан (core/exec_book.rs); стакан потока устарел — снимок REST (в строке хронологии).
//! Решение 238 (ревью, п. 3): стакан котировки (trade_ops::deal_quote) передаётся ордеру (trade_seen) — порог убытка
//! и лимит IOC первой попытки считаются по одному снимку.
//! Решение 235: продажа кнопкой (BUTTON) — до конца в режиме кнопки: повтор после отказа до отправки, остаток после
//! частичной продажи и газ BNB — manual_sell.rs (без порогов убытка и «выгодно»); исчерпание повторов — алерт; итог
//! кнопки — после закрытия, с возвратом USDT (manual_outcome.rs).
//! Решение 247: продажа DEX откатилась на пробе по minOut — адаптер пере-меряет реальный выход и повторяет с minOut от
//! него, если выход не ниже границы (TradeRequest.remeasure): кнопка — всегда (без порога), режим застрявшей
//! монеты — по своему порогу убытка (market_guarded ← stuck.rs, relay_here.rs; граница — core/remeasure.rs). Дополнение
//! 10.11 (Тигран: согласовано): «Продать и перегнать» (manual_relay.rs) — все её продажи кнопкой (guard_of: Guard::Any);
//! продажа на цели сразу после автоматического «Перегона» (relay.rs → sell_after_relay) — с пере-замером по условию
//! «выгодно» (порог убытка 0). Дополнение 10.11 (t78, Тигран: согласовано): по тому же порогу «итог сделки ≥ 0»
//! (remeasure::PROFITABLE) — и первая продажа сделки и «Вернуть боту» (sell_or_stuck), продажа после частичной покупки
//! (executor.rs → market_guarded), «Ожидание» по истечении stuck.wait_ms (stuck.rs). t81 (дополнение к 247 п. Г, Тигран
//! 10.11: согласовано): у этих путей t78 — порог remeasure::PROFITABLE_OPEN (та же граница; продажа может идти и
//! агрегатором DEX, если он включён и его маршрут лучше); sell_after_relay — PROFITABLE (только свой сборщик, как было).
//! Решение 268 (Тигран 14.11, живой случай FAR 13.11 — MEXC 30087): лимитный ордер продажи отклонён биржей «цена вне
//! разрешённого диапазона» (код шлюза PriceOutOfRange) — один раз на сделку: по свежему стакану продажа выгодна (у
//! кнопки — без этой проверки) — рыночный ордер, иначе режим застрявшей монеты (sell_range.rs, sell_range_run.rs;
//! sell.market_on_price_range). Продажа на бирже закончилась алертом — новые входы с продажей на ней закрываются
//! (sell_block.rs; sell.block_entry_on_halt). Обе настройки не заданы — как раньше.
//! Кто вызывает: executor.rs, stuck.rs, manual.rs, manual_halted.rs, manual_sell.rs, manual_relay.rs, relay_here.rs.
//! От чего зависит: trade_ops.rs, fills.rs, ledger.rs, stuck.rs, manual.rs, deal.rs, sell_slip.rs, sell_unsent.rs,
//! manual_outcome.rs, risk_count.rs, remeasure.rs, sell_range.rs, sell_range_run.rs, sell_block.rs.
//! Настройки: через sell_unsent.rs — stuck.unsent_retries_max (решение 110), stuck.reject_retry_pause_ms (решение 154);
//! через sell_range.rs — sell.market_on_price_range; через sell_block.rs — sell.block_entry_on_halt (решение 268).

use rust_decimal::Decimal;

use crate::gateway::errors::ErrorCode;

use crate::core::types::{Quote, Side};

use super::deal::{Deal, Step};
use super::engine::Core;
use super::remeasure::Guard;
use super::stuck_reason::StuckReason;

/// Причина продажи кнопкой «Продать сейчас» / «Продать и перегнать» (manual.rs, manual_halted.rs, manual_relay.rs).
/// Решение 110: продажу, которую нажал человек, бот не уводит в режим застрявшей монеты — сбой сразу алертом; решение
/// 229 — кроме отказа ДО отправки (проба откатилась, нет маршрута …): его бот повторяет сам; решение 235 — повтор,
/// остаток после частичной продажи и газ BNB у кнопки — в режиме кнопки, без порогов убытка (manual_sell.rs).
pub const BUTTON: &str = "кнопка «Продать сейчас»";

/// Выгодно ли продать за value (USDT чистыми): итог сделки не ниже нуля (решение 47).
pub fn profitable(deal: &Deal, value: Decimal) -> bool {
    deal.result_if(value) >= Decimal::ZERO
}

/// Продать сейчас, если выгодно, иначе — режим застрявшей монеты (первая продажа сделки — executor.rs, «Вернуть боту» —
/// manual_return.rs). Пере-замер продажи DEX при откате «мало на выходе» — в пределах того же условия «выгодно»: итог
/// сделки ≥ 0, порог убытка 0 (remeasure::PROFITABLE; решение 247, дополнение 10.11, t78 — Тигран: согласовано); итог
/// ниже — отказ продажи, дальше как раньше. t81 (дополнение к 247 п. Г, Тигран 10.11: согласовано): порог
/// PROFITABLE_OPEN — исполнить может и агрегатор DEX, если включён и его маршрут лучше (без пере-замера). Ошибок нет.
pub async fn sell_or_stuck(core: &Core, deal: Deal) {
    sell_or_stuck_guarded(core, deal, super::remeasure::PROFITABLE_OPEN).await
}

/// Продажа на площадке-цели сразу после автоматического «Перегона» (relay.rs): как sell_or_stuck — с пере-замером по
/// условию «выгодно» (remeasure::PROFITABLE; решение 247, дополнение 10.11 — Тигран: согласовано). t81: порог
/// PROFITABLE, не PROFITABLE_OPEN — путь из решения 247 (t77), только свой сборщик (п. Г). Ошибок нет.
pub async fn sell_after_relay(core: &Core, deal: Deal) {
    sell_or_stuck_guarded(core, deal, super::remeasure::PROFITABLE).await
}

/// Выгодно — продажа с пере-замером по порогу `g`, иначе — режим застрявшей монеты.
/// Принимает: ядро, сделку, порог пере-замера. Ошибок нет.
async fn sell_or_stuck_guarded(core: &Core, deal: Deal, g: Guard) {
    let value = super::trade_ops::deal_quote(core, &deal).await;
    match value {
        Ok((v, q, b)) if profitable(&deal, v) => {
            let f = super::remeasure::of(&deal, v, deal.qty * q.avg_price, g);
            sell(core, deal, q, b, "выгодно", f).await
        }
        // Решение 220: в режим застрявшей монеты — с настоящей причиной.
        Ok(_) => Box::pin(super::stuck::handle(core, deal, StuckReason::Unprofitable)).await,
        Err(e) => {
            let why = StuckReason::NoQuote(e.message);
            Box::pin(super::stuck::handle(core, deal, why)).await
        }
    }
}

/// Продажа по рынку (по текущей котировке; худшая цена — trade_ops). Решение 247: кнопка человека (BUTTON) — с
/// пере-замером без порога (remeasure::Guard::Any), прочие причины — без пере-замера (автоматика продаёт через
/// market_guarded со своим порогом; t78 — по коду других вызовов market, кроме кнопки, нет).
/// Ошибок нет: не вышло — сделка остановлена, монеты остаются на площадке (видно на экране «Сделки»).
pub async fn market(core: &Core, deal: Deal, why: &str) {
    market_guarded(core, deal, why, guard_of(why)).await
}

/// Порог пере-замера продажи по рынку по её причине (решение 247): кнопка человека (BUTTON — «Продать сейчас», и
/// продажи «Продать и перегнать»: здесь, на цели после перегона, после несостоявшегося перегона) — Guard::Any (без
/// порога, решение 235); прочие — Guard::Off. Открыта для тестов. Ошибок нет.
pub fn guard_of(why: &str) -> Guard {
    if why == BUTTON {
        Guard::Any
    } else {
        Guard::Off
    }
}

/// Как market, с пере-замером продажи DEX при откате по minOut (решение 247): граница реального выхода — по порогу
/// `g` и свежей котировке (remeasure::floor). Принимает: ядро, сделку, причину, порог. Ошибок нет (как market).
pub async fn market_guarded(core: &Core, deal: Deal, why: &str, g: Guard) {
    // Решение 201: продажа ждёт конец паузы площадки после 429 (exec.rate_limit_wait_max_ms), а не отказывает сразу.
    super::patience_scope::with(core, async {
        match super::trade_ops::deal_quote(core, &deal).await {
            Ok((v, q, b)) => {
                let f = super::remeasure::of(&deal, v, deal.qty * q.avg_price, g);
                sell(core, deal, q, b, why, f).await
            }
            Err(e) => {
                let mut deal = deal;
                let text = format!("продажа по рынку ({why}) невозможна: {e}");
                super::ledger::finish(core, &mut deal, Step::Halted, &text).await;
                super::manual_outcome::failed(core, &deal, why, &text).await;
            }
        }
    })
    .await
}

/// Строка хронологии (решение 227): котировка продажи (средняя, исполнимо из запрошенного, возраст котировки) и худшая
/// цена ордера со стаканом, по которому посчитана (возраст стакана ядра; у DEX — от средней с допуском).
/// Принимает: котировку, количество ордера, худшую цену (None — до расчёта не дошло). Ошибок нет.
fn quote_note(q: &Quote, qty: Decimal, w: Option<&super::trade_ops::Worst>) -> String {
    let age = (chrono::Utc::now() - q.at).num_milliseconds();
    let worst = match w {
        Some(w) => match w.book_age_ms {
            // Решение 238: стакан потока устарел — котировка и худшая цена по одному снимку REST.
            Some(b) if w.rest => format!(
                "худшая цена ордера {} по снимку стакана REST возрастом {b} мс (стакан потока устарел)",
                w.price
            ),
            Some(b) => format!("худшая цена ордера {} по стакану возрастом {b} мс", w.price),
            None => format!(
                "худшая цена ордера {} (средняя котировки с допуском DEX)",
                w.price
            ),
        },
        None => "худшая цена ордера не посчитана".to_string(),
    };
    format!(
        "Котировка продажи на {}: средняя {}, исполнимо {} из {qty} (котировке {age} мс); {worst}",
        q.venue.0, q.avg_price, q.fillable_qty
    )
}

/// Продажа всего количества по котировке q. Принимает: сделку, котировку, стакан котировки (решение 238, ревью п. 3:
/// худшая цена первой попытки — по нему; None — DEX), причину (для хронологии), границу пере-замера продажи DEX
/// (решение 247, remeasure::of; None — без пере-замера).
async fn sell(
    core: &Core,
    mut deal: Deal,
    q: Quote,
    book: Option<super::exec_book::Book>,
    why: &str,
    floor: Option<crate::core::types::Remeasure>,
) {
    let (v, a) = (deal.at.clone(), deal.direction.asset.clone());
    let mut seen = None;
    let mut gas = Vec::new();
    let mut res = super::trade_ops::trade_seen(
        core,
        &deal.id,
        &v,
        &a,
        Side::Sell,
        deal.qty,
        q.route.clone(),
        q.avg_price,
        // Решение 262: худший уровень плана — только у покупки; продажа — по текущему стакану.
        None,
        None,
        book,
        &mut seen,
        Some(&mut gas),
        floor,
        false,
    )
    .await;
    // Решение 236, дополнение: approve, чаевые, газ неудачных попыток — в затраты сделки.
    super::deal_gas::charge(core, &mut deal, &v, &mut gas).await;
    // Решение 227: котировка продажи и стакан худшей цены — в хронологию сделки (разбор живых случаев вроде JCT).
    deal.note(quote_note(&q, deal.qty, seen.as_ref()));
    // Решение 268 (Тигран 14.11): биржа отклонила лимитный ордер продажи «цена вне разрешённого диапазона» — один раз
    // на сделку: свежий стакан и, если продажа выгодна (кнопка человека — без этой проверки), рыночный ордер; его итог
    // (факт или ошибка) разбирается ниже тем же кодом, что у лимитного. Невыгодно или нет котировки — режим застрявшей
    // монеты (как sell_or_stuck). Настройка выключена, DEX, повтор уже был — ветки нет, всё как раньше (sell_range.rs).
    if let Err(e) = &res
        && super::sell_range::wanted(core, &deal, e)
    {
        // Итог ветки кладётся в то же место, что итог лимитного ордера (состояние задачи продажи не растёт).
        let out = super::sell_range_run::run(core, &mut deal, e, why).await;
        match out {
            super::sell_range::Outcome::Sold(r) => res = Ok(*r),
            super::sell_range::Outcome::Failed(e) => res = Err(e),
            super::sell_range::Outcome::Stuck(reason) => {
                return super::sell_range_run::stuck(core, deal, reason).await;
            }
        }
    }
    match res {
        Ok(r) => {
            super::fills::trade(core, &mut deal, &r).await;
            deal.sells.push(r.clone());
            // Решение 177: продано — запись, с которой продавали, больше не нужна.
            deal.alert_was = None;
            let text = crate::notify::human::Msg::new(crate::notify::human::OK, "Продано")
                .line(format!(
                    "{} {} на {} по {}",
                    crate::notify::human::qty(r.filled_qty),
                    a.ticker,
                    crate::notify::human::venue(&v.0),
                    crate::notify::human::qty(r.avg_price)
                ))
                .kv(
                    "Выручка",
                    crate::notify::human::usd(r.filled_qty * r.avg_price),
                )
                .id(&deal.id)
                .render();
            core.inner
                .deps
                .notify("deal.sell_filled", Some(&v.0), &text)
                .await;
            // Решение 66: остаток меньше шага или минимума ордера — пыль, а не застрявшая монета.
            super::dust::settle(core, &mut deal, r.avg_price).await;
            // Решение 235: остаток продажи кнопкой — снова по рынку кнопкой (manual_sell.rs), итог кнопки — в конце.
            if deal.qty > Decimal::ZERO && why == BUTTON {
                return Box::pin(super::manual_sell::remainder(core, deal)).await;
            }
            if deal.qty > Decimal::ZERO {
                super::manual_outcome::sold(core, &deal, why, &r, deal.qty).await;
                deal.note(format!("Продано не всё, остаток {}", deal.qty));
                super::ledger::save(core, &deal).await;
                // Решение 177: остаток продаёт автоматика (режим застрявшей монеты), не ручная команда.
                let left = StuckReason::Partial(deal.qty);
                return Box::pin(super::risk_count::auto(super::stuck::handle(
                    core, deal, left,
                )))
                .await;
            }
            // Возврат USDT на площадку покупки — общий шаг закрытия (ledger::finish, решение 138).
            super::ledger::finish(core, &mut deal, Step::Closed, &format!("продано ({why})")).await;
            // Решение 235: итог кнопки — после закрытия: в нём и возврат USDT (начат / не нужен / не вернётся).
            super::manual_outcome::sold(core, &deal, why, &r, Decimal::ZERO).await;
            // Решение 148: факт продажи против плана — серия хуже плана уводит монету в чёрный список.
            super::sell_slip::after_close(core, &deal).await;
        }
        // BNB на DEX (решение 62): отказ резерва на газ (InsufficientBalance) — монета на месте, просто газ продажи
        // сейчас не помещается сверх резерва: «застрявшая монета» (stuck.rs), а не остановка сделки.
        Err(e)
            if e.code == Some(ErrorCode::InsufficientBalance)
                && a.ticker == super::convert::GAS_TICKER =>
        {
            // Решение 235: у кнопки — алерт с точной причиной, не режим застрявшей монеты (там пороги убытка).
            if why == BUTTON {
                return Box::pin(super::manual_sell::gas(core, deal, &e)).await;
            }
            deal.note(format!("продажа не прошла: {e} — застрявшая монета"));
            super::ledger::save(core, &deal).await;
            let why_not =
                format!("{e} — газ продажи не помещается сверх резерва, режим застрявшей монеты");
            super::manual_outcome::failed(core, &deal, why, &why_not).await;
            let gas = StuckReason::GasReserve(e.message.clone());
            Box::pin(super::risk_count::auto(super::stuck::handle(
                core, deal, gas,
            )))
            .await
        }
        // Исход продажи неизвестен (транзакция могла исполниться) — ни продавать, ни перегонять: ручная проверка.
        Err(e) if e.outcome_unknown() => {
            let text = format!("исход продажи неизвестен: {e} — проверьте вручную");
            super::ledger::finish(core, &mut deal, Step::Halted, &text).await;
            super::manual_outcome::failed(core, &deal, why, &text).await;
        }
        // Решение 100: продажа не прошла (откат, не попала в блок, цена хуже допустимой, отказ площадки) — монеты на
        // месте; бот сам не повторяет (стартовое правило), а ставит алерт человеку: «Продать сейчас» (по реальному
        // балансу), «Беру на себя», «Продано вручную» — в интерфейсе и кнопками Telegram.
        // Решения 110 и 154: продажа не ушла в сеть (отказ до отправки; или ни один канал не принял, и узел подтвердил,
        // что транзакции нет ни в блоке, ни в пуле) — денег и газа не стоила, стартовое правило («ордер отправлен и не
        // исполнился») не про неё: повтор через режим застрявшей монеты, не больше stuck.unsent_retries_max раз
        // (sell_unsent.rs). Строку хронологии об отказе пишет алерт (manual::raise_alert) — без дубля (решение 154).
        // Решение 229: так же — не попала в блок, а nonce занят нашей отменой в блоке (исход известен), и отказ до
        // отправки после кнопки человека; исчерпание такой цепочки — продажа здесь по порогу (relay_here.rs). Повтор
        // после кнопки — уже автоматика (risk_count::auto), итог кнопки — «не продано, бот повторит сам».
        Err(e) => match super::sell_unsent::decide(core, &deal, &e, why) {
            // Решение 235: повтор кнопки — снова продажа по рынку кнопкой (manual_sell.rs), не режим застрявшей
            // монеты с его порогами; признак задачи остаётся ручным, итог кнопки — один, в конце.
            super::sell_unsent::Retry::Go { pause_ms } if why == BUTTON => {
                Box::pin(super::manual_sell::retry(core, deal, &e, pause_ms)).await
            }
            super::sell_unsent::Retry::Go { pause_ms } => {
                super::manual_outcome::failed(
                    core,
                    &deal,
                    why,
                    &format!("{e} — бот повторит продажу сам"),
                )
                .await;
                let job = super::sell_unsent::retry(core, deal, &e, why, pause_ms);
                Box::pin(super::risk_count::auto(job)).await
            }
            super::sell_unsent::Retry::Here { max } => {
                let text = format!("{e} — повторы исчерпаны, бот продаёт здесь по порогу");
                super::manual_outcome::failed(core, &deal, why, &text).await;
                Box::pin(super::risk_count::auto(super::sell_unsent::here(
                    core, deal, &e, max,
                )))
                .await
            }
            r => {
                // Решение 268: продажа на бирже остановлена алертом — закрыть новые входы с продажей на ней
                // (sell.block_entry_on_halt; не задана или нет — ничего не делает).
                super::sell_block::close(core, &deal, &e).await;
                // Решение 177: итог кнопки — после алерта (запись уже обновлена или поставлена).
                let d = deal.clone();
                super::sell_unsent::alert(core, deal, &e, &r).await;
                super::manual_outcome::failed(core, &d, why, &e.message).await;
            }
        },
    }
}
