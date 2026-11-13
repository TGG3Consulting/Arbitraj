//! Файл: core/precheck.rs
//! Что делает: предпроверка сразу перед покупкой (шаг 5 ТЗ): стоп-условия (risk.rs); статусы вывода на A
//! и депозита на B — перечитываются напрямую из API бирж; покупка через API на A и продажа на B — по свежему справочнику
//! пар (pairs_fresh.rs); у биржи B — адрес депозита монеты (решение 118, precheck_deposit.rs); BNB на газ у DEX-сторон не ниже
//! risk.min_bnb_balance; затем полный расчёт заново (profit.rs) по свежим стаканам и пулам; биржа B примет ордер
//! продажи на объём плана (решение 134, precheck_sell.rs), продажа DEX B — симуляцией настоящей продажи кошельком сделки
//! с пересчётом по её выходу (решение 234, precheck_dex_sell.rs); на бирже A хватит средств, которые можно вывести (решение 150,
//! precheck_withdraw.rs).
//! Решение 265: время шагов — «Время по шагам (предпроверка): …» и этапы пересчёта «Время по шагам (пересчёт с
//! финальной котировкой, всего N мс): …» (step_timing.rs) — в журнал core.precheck и хронологию сделки (executor.rs).
//! Кто вызывает: executor.rs.
//! От чего зависит: step_timing.rs, risk.rs, profit.rs, convert.rs, trade_ops.rs (вид площадки), deps.rs (шлюз).
//! Настройки: risk.min_bnb_balance, venue.symbols_stale_s (+ всё, что читают risk.rs и profit.rs).

use rust_decimal::Decimal;

use crate::core::types::{Direction, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;

use super::costs::{Calc, CostLine};
use super::engine::{Core, wr};
use super::profit::{self, Verdict};
use super::step_timing::Steps;

/// Статус ввода или вывода монеты по сети BSC прямо из API биржи. Ошибка — текст причины.
async fn transfer_open(
    core: &Core,
    v: &VenueId,
    d: &Direction,
    withdraw: bool,
) -> Result<(), String> {
    let gw = core.inner.deps.gw.get(v).await.map_err(|e| e.message)?;
    // Решение 113: справочник одной монеты (BingX — config/getall?coin=; у кого запроса одной монеты нет — весь
    // справочник с фильтром, как раньше).
    // Решение 211: справочник площадки — по её тикеру монеты; сеть ищется по адресу.
    let on = super::venue_ticker::ticker_on(core, &d.asset, v);
    let list = gw
        .coin_networks_of(&on)
        .await
        .map_err(|e| format!("{}: справочник сетей: {}", v.0, e.message))?;
    // Нативная монета ↔ обёртка (решение 62): тот же справочник, что у ядра (native.rs).
    let list = super::native::map(core, v, list);
    let open = list
        .iter()
        .find(|i| i.asset.bsc_contract.is_some() && i.asset.bsc_contract == d.asset.bsc_contract)
        .map(|i| {
            if withdraw {
                i.withdraw_enabled
            } else {
                i.deposit_enabled
            }
        });
    // Свежие записи этой монеты — в справочник ядра вместо её старых; остальные монеты биржи не трогаются (решение 117:
    // после решения 113 здесь приходит справочник ОДНОЙ монеты, а он целиком заменял справочник биржи — остальные
    // монеты теряли сеть BSC до следующего опроса). Справочника биржи ещё нет — не пишем (его соберёт registry_sync).
    {
        let mut nets = wr(&core.inner.networks);
        if let Some(all) = nets.get_mut(v).map(std::sync::Arc::make_mut) {
            // Решение 211: записи этой монеты площадки — по её тикеру (и именам из свежего ответа после native::map);
            // показное имя ядра у этой биржи может быть другой монетой — по нему не чистим.
            let mut gone: std::collections::HashSet<String> =
                list.iter().map(|i| i.asset.ticker.clone()).collect();
            gone.insert(on.clone());
            all.retain(|i| !gone.contains(&i.asset.ticker));
            all.extend(list);
        }
    }
    core.request_rebuild();
    match open {
        Some(true) => Ok(()),
        Some(false) => Err(format!(
            "{}: {} {} закрыт",
            v.0,
            d.asset.ticker,
            if withdraw { "вывод" } else { "ввод" }
        )),
        None => Err(format!(
            "{}: у {} нет сети BSC с тем же адресом контракта",
            v.0, d.asset.ticker
        )),
    }
}

/// Сторона сделки по справочнику пар биржи (pairs_fresh.rs): на A (buy = true) покупка через API разрешена, на B —
/// продажа; справочник не старше venue.symbols_stale_s. Направление могло быть поставлено до смены флагов —
/// здесь оно отсекается до ордера. Ошибка — текст причины.
fn pair_side(core: &Core, v: &VenueId, d: &Direction, buy: bool) -> Result<(), String> {
    let limit = super::pairs_fresh::limit(&core.inner.deps.settings, v);
    let t = super::venue_ticker::ticker_on(core, &d.asset, v);
    let market = super::engine::rd(&core.inner.market);
    super::pairs_fresh::side(&market, v, &t, buy, &limit, chrono::Utc::now())
}

/// BNB на газ у DEX-площадки: свободный нативный BNB ≥ резерв risk.min_bnb_balance + газ транзакций сделки на ней
/// (need; сверка балансов — 0). Решение 132: кошелёк — строки маппинга направления. Принимает: ядро, направление,
/// площадку, газ сделки. Ошибка — текст (мало BNB, резерв не задан, нет кошелька в маппинге, баланс не получен).
pub async fn bnb_ok(core: &Core, d: &Direction, v: &VenueId, need: Decimal) -> Result<(), String> {
    let min = core
        .inner
        .deps
        .settings
        .decimal("risk.min_bnb_balance", At::default())
        .map_err(|e| e.message)?;
    let gw = super::dex_account::venue_dir(&core.inner.deps, d, v).await?;
    // Решение 113: только BNB (у DEX — eth_getBalance), а не балансы всех токенов кошелька (~11 с на вызов).
    let have = gw
        .balance(super::convert::GAS_TICKER)
        .await
        .map_err(|e| format!("{}: балансы: {}", v.0, e.message))?
        .map_or(Decimal::ZERO, |x| x.free);
    bnb_enough(&v.0, have, min, need)
}

/// Хватает ли BNB на газ: свободный BNB не меньше минимума risk.min_bnb_balance плюс газ сделки. Принимает: площадку,
/// свободный BNB, минимум, газ сделки. Ошибка — текст причины. Вызывают bnb_ok и сверка (reconcile.rs — по уже
/// полученным балансам, без второго запроса).
pub fn bnb_enough(who: &str, have: Decimal, min: Decimal, need: Decimal) -> Result<(), String> {
    if have < min + need {
        // Решение 164: числа для глаз (количество BNB — 4 значащие цифры).
        use crate::notify::human::qty;
        return Err(format!(
            "{who}: BNB на газ {} — меньше нужного {} (запас {} + газ сделки {})",
            qty(have),
            qty(min + need),
            qty(min),
            qty(need)
        ));
    }
    Ok(())
}

/// Газ (BNB) транзакций сделки на одной площадке: строки расчёта её стадий с газом в BNB (газ обмена, разрешений,
/// перевода, обёртки, чаевые; у «возврат USDT» — газ перевода USDT, convert::usdt_return). Ошибок нет.
pub fn gas_need(costs: &[CostLine], stages: &[&str]) -> Decimal {
    costs
        .iter()
        .filter(|l| stages.contains(&l.stage) && l.asset == super::convert::GAS_TICKER)
        .filter(|l| super::convert::is_gas(l.kind))
        .map(|l| l.amount)
        .sum()
}

/// Предпроверка. Возвращает: свежий расчёт для сделки и строки «Время по шагам» (шаги предпроверки; этапы пересчёта —
/// решение 265) для хронологии сделки. Ошибки: текст причины отказа (с временем шагов).
/// Решение 111: время каждого шага (справочник сетей биржи, баланс BNB, пересчёт с финальной котировкой) — в текст
/// отказа и в журнал при успехе: 02.11 у RVV между кандидатом и отказом прошло 16 с, а на что — по журналу не видно.
/// Решение 265: при успехе в журнал и хронологию — и этапы пересчёта по шагам (step_timing.rs, calc_steps.rs).
pub async fn run(core: &Core, d: &Direction) -> Result<(Calc, Vec<String>), String> {
    let mut t = Steps::default();
    let mut calc = None;
    let res = steps(core, d, &mut t, &mut calc).await;
    let lines: Vec<String> = std::iter::once(t.text("предпроверка"))
        .chain(calc)
        .collect();
    if res.is_ok() {
        core.inner
            .deps
            .log(
                crate::storage::journal::Level::Info,
                "core.precheck",
                Some(&d.buy_on.0),
                format!(
                    "{} {} → {}: предпроверка пройдена. {}",
                    d.asset.ticker,
                    d.buy_on.0,
                    d.sell_on.0,
                    lines.join(". ")
                ),
                None,
            )
            .await
    }
    res.map(|c| (c, lines))
        .map_err(|e| format!("{e} ({})", t.text("предпроверка")))
}

/// Шаги предпроверки с замером времени. Ошибки: текст причины отказа.
/// Решение 265: `calc` — строка этапов пересчёта (заполняется, если до пересчёта дошли).
async fn steps(
    core: &Core,
    d: &Direction,
    t: &mut Steps,
    calc: &mut Option<String>,
) -> Result<Calc, String> {
    use std::time::Instant;
    let at = Instant::now();
    let r = super::risk::can_open(core, d);
    t.mark("стоп-условия", at);
    r?;
    for (v, withdraw) in [(&d.buy_on, true), (&d.sell_on, false)] {
        match super::trade_ops::kind(core, v)? {
            VenueKind::Cex => {
                let at = Instant::now();
                let r = transfer_open(core, v, d, withdraw).await;
                t.mark(format!("справочник сетей {}", v.0), at);
                r?;
                pair_side(core, v, d, withdraw)?;
                // Решение 118 (Я-5): у биржи-получателя адрес депозита — до покупки, а не после.
                if !withdraw {
                    let at = Instant::now();
                    let r = super::precheck_deposit::ready(core, d).await;
                    t.mark(format!("адрес депозита {}", v.0), at);
                    r?;
                }
            }
            VenueKind::Dex => {
                let at = Instant::now();
                let r = bnb_ok(core, d, v, Decimal::ZERO).await;
                t.mark(format!("баланс BNB {}", v.0), at);
                if let Err(e) = r {
                    core.inner.deps.notify("risk.low_bnb", Some(&v.0), &e).await;
                    return Err(e);
                }
            }
        }
    }
    let at = Instant::now();
    // Будущее расчёта большое — в куче: обёртки замера иначе копируют его на стек (переполнение в отладочной сборке).
    let (verdict, m) = super::step_timing::measure(Box::pin(replan(core, d))).await;
    t.mark("пересчёт с финальной котировкой", at);
    *calc = Some(m.calc_text("пересчёт с финальной котировкой"));
    let c = match verdict {
        Verdict::Accept(c) => c,
        Verdict::Reject { reason, .. } => return Err(format!("пересчёт перед покупкой: {reason}")),
    };
    // Решение 134: биржа B примет ордер продажи на объём плана — до покупки (precheck_sell.rs). Решение 234: у DEX B —
    // симуляция настоящей продажи кошельком сделки, расчёт — по её выходу (precheck_dex_sell.rs).
    let at = Instant::now();
    let r = super::precheck_sell::check(core, d, c).await;
    t.mark(format!("проверка продажи {}", d.sell_on.0), at);
    let c = r?;
    // Решение 150: на бирже A хватит выводимого (без предварительно зачисленных депозитов) — до покупки.
    let at = Instant::now();
    let r = super::precheck_withdraw::check(core, d, &c).await;
    t.mark(format!("удержания вывода {}", d.buy_on.0), at);
    r?;
    // Решение 62: газ всех транзакций сделки на DEX — сверх резерва, иначе после покупки BNB перевод на биржу
    // упадёт с InsufficientBalance и монета застрянет. A — покупка и перевод, B — продажа и возврат USDT.
    for (v, stages) in [
        (&d.buy_on, ["A", "перевод"]),
        (&d.sell_on, ["B", super::costs_types::RETURN_STAGE]),
    ] {
        if super::trade_ops::kind(core, v)? != VenueKind::Dex {
            continue;
        }
        let at = Instant::now();
        let r = bnb_ok(core, d, v, gas_need(&c.costs, &stages)).await;
        t.mark(format!("газ BNB {}", v.0), at);
        if let Err(e) = r {
            core.inner.deps.notify("risk.low_bnb", Some(&v.0), &e).await;
            return Err(e);
        }
    }
    Ok(c)
}

/// Что делает: пересчёт живой сделки перед покупкой — всегда в режиме Live (решение 74): виртуального бюджета у сделки не
/// бывает, даже если сухой режим включили сейчас. Принимает: ядро, направление. Возвращает: Verdict. Ошибок нет.
pub async fn replan(core: &Core, d: &Direction) -> Verdict {
    profit::decide(core, d, &super::dry::Mode::Live).await
}
