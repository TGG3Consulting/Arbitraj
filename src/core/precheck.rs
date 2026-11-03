//! Файл: core/precheck.rs
//! Что делает: предпроверка сразу перед покупкой (шаг 5 ТЗ): стоп-условия (risk.rs); статусы вывода на A
//! и депозита на B — перечитываются напрямую из API бирж; покупка через API на A и продажа на B — по свежему справочнику
//! пар (pairs_fresh.rs); у биржи B — адрес депозита монеты (решение 118, precheck_deposit.rs); BNB на газ у DEX-сторон не ниже
//! risk.min_bnb_balance; затем полный расчёт заново (profit.rs) по свежим стаканам и пулам.
//! Кто вызывает: executor.rs.
//! От чего зависит: risk.rs, profit.rs, convert.rs, trade_ops.rs (вид площадки), deps.rs (шлюз).
//! Настройки: risk.min_bnb_balance, venue.symbols_stale_s (+ всё, что читают risk.rs и profit.rs).

use rust_decimal::Decimal;

use crate::core::types::{Direction, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;

use super::costs::{Calc, CostLine};
use super::engine::{Core, wr};
use super::profit::{self, Verdict};

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
    let list = gw
        .coin_networks_of(&d.asset.ticker)
        .await
        .map_err(|e| format!("{}: справочник сетей: {}", v.0, e.message))?;
    // Нативная монета ↔ обёртка (решение 62): тот же справочник, что у ядра (native.rs).
    let list = super::native::map(core, v, list);
    let open = list
        .iter()
        .find(|i| {
            i.asset.ticker == d.asset.ticker
                && i.asset.bsc_contract.is_some()
                && i.asset.bsc_contract == d.asset.bsc_contract
        })
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
            all.retain(|i| i.asset.ticker != d.asset.ticker);
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
    let market = super::engine::rd(&core.inner.market);
    super::pairs_fresh::side(&market, v, &d.asset.ticker, buy, &limit, chrono::Utc::now())
}

/// BNB на газ у DEX-площадки: свободный нативный BNB ≥ резерв risk.min_bnb_balance + газ транзакций сделки на ней
/// (need; сверка балансов — 0). Ошибка — текст (мало BNB, резерв не задан или баланс не получен).
pub async fn bnb_ok(core: &Core, v: &VenueId, need: Decimal) -> Result<(), String> {
    let min = core
        .inner
        .deps
        .settings
        .decimal("risk.min_bnb_balance", At::default())
        .map_err(|e| e.message)?;
    let gw = core.inner.deps.gw.get(v).await.map_err(|e| e.message)?;
    // Решение 113: только BNB (у DEX — eth_getBalance), а не балансы всех токенов кошелька (~11 с на вызов).
    let have = gw
        .balance(super::convert::GAS_TICKER)
        .await
        .map_err(|e| format!("{}: балансы: {}", v.0, e.message))?
        .map_or(Decimal::ZERO, |x| x.free);
    bnb_enough(v, have, min, need)
}

/// Хватает ли BNB на газ: свободный BNB не меньше минимума risk.min_bnb_balance плюс газ сделки. Принимает: площадку,
/// свободный BNB, минимум, газ сделки. Ошибка — текст причины. Вызывают bnb_ok и сверка (reconcile.rs — по уже
/// полученным балансам, без второго запроса).
pub fn bnb_enough(v: &VenueId, have: Decimal, min: Decimal, need: Decimal) -> Result<(), String> {
    if have < min + need {
        return Err(format!(
            "{}: BNB на газ {have} меньше минимума {min} + газ сделки {need}",
            v.0
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

/// Предпроверка. Возвращает: свежий расчёт для сделки. Ошибки: текст причины отказа.
/// Решение 111: время каждого шага (справочник сетей биржи, баланс BNB, пересчёт с финальной котировкой) — в текст
/// отказа и в журнал при успехе: 02.11 у RVV между кандидатом и отказом прошло 16 с, а на что — по журналу не видно.
pub async fn run(core: &Core, d: &Direction) -> Result<Calc, String> {
    let mut t = Timing::default();
    let res = steps(core, d, &mut t).await;
    if res.is_ok() {
        core.inner
            .deps
            .log(
                crate::storage::journal::Level::Info,
                "core.precheck",
                Some(&d.buy_on.0),
                format!(
                    "{} {} → {}: предпроверка пройдена ({})",
                    d.asset.ticker,
                    d.buy_on.0,
                    d.sell_on.0,
                    t.text()
                ),
                None,
            )
            .await
    }
    res.map_err(|e| format!("{e} ({})", t.text()))
}

/// Время шагов предпроверки: (что, мс).
#[derive(Default)]
struct Timing(Vec<(String, u128)>);

impl Timing {
    /// Запоминает шаг. Принимает: название, момент начала. Ошибок нет.
    fn mark(&mut self, what: String, from: std::time::Instant) {
        self.0.push((what, from.elapsed().as_millis()));
    }

    /// «время: справочник сетей bingx 820 мс, пересчёт 14100 мс». Ошибок нет.
    fn text(&self) -> String {
        let parts: Vec<String> = self
            .0
            .iter()
            .map(|(w, ms)| format!("{w} {ms} мс"))
            .collect();
        format!("время: {}", parts.join(", "))
    }
}

/// Шаги предпроверки с замером времени. Ошибки: текст причины отказа.
async fn steps(core: &Core, d: &Direction, t: &mut Timing) -> Result<Calc, String> {
    use std::time::Instant;
    super::risk::can_open(core, d)?;
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
                let r = bnb_ok(core, v, Decimal::ZERO).await;
                t.mark(format!("баланс BNB {}", v.0), at);
                if let Err(e) = r {
                    core.inner.deps.notify("risk.low_bnb", Some(&v.0), &e).await;
                    return Err(e);
                }
            }
        }
    }
    let at = Instant::now();
    let verdict = replan(core, d).await;
    t.mark("пересчёт с финальной котировкой".into(), at);
    let c = match verdict {
        Verdict::Accept(c) => c,
        Verdict::Reject { reason, .. } => return Err(format!("пересчёт перед покупкой: {reason}")),
    };
    // Решение 62: газ всех транзакций сделки на DEX — сверх резерва, иначе после покупки BNB перевод на биржу
    // упадёт с InsufficientBalance и монета застрянет. A — покупка и перевод, B — продажа и возврат USDT.
    for (v, stages) in [
        (&d.buy_on, ["A", "перевод"]),
        (&d.sell_on, ["B", "возврат USDT"]),
    ] {
        if super::trade_ops::kind(core, v)? != VenueKind::Dex {
            continue;
        }
        let at = Instant::now();
        let r = bnb_ok(core, v, gas_need(&c.costs, &stages)).await;
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
