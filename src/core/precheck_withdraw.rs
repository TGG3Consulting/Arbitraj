//! Файл: core/precheck_withdraw.rs
//! Что делает: предпроверка до покупки (решение 150, Тигран 04.11.2026): хватит ли на бирже A средств, которые МОЖНО
//! ВЫВЕСТИ. Живой случай 04.11 (сделка 20261104173851521BABYDOGE, mexc → bsc_dex): покупка на MEXC прошла, вывод —
//! отказ «код 16021: Some pre-crediting assets are temporarily unavailable for withdrawal». Предварительно зачисленные
//! депозиты торгуются, но не выводятся; как именно MEXC ограничивает вывод, в документации нет. Строгое правило
//! Тиграна до ответа MEXC: из того, что можно вывести, вычитается ВСЯ стоимость удержаний в USDT, в какой бы монете они
//! ни были: можно вывести = свободная котируемая монета на A − удержания в USDT; меньше суммы покупки — сделки нет.
//! Проверка — только если A биржа (после покупки будет вывод с неё). Удержаний нет — ничего не меняет; у бирж без
//! такого механизма контракт отвечает пусто без запросов. Сбой запроса удержаний или нет цены удержания — сделки нет.
//! Сумма покупки — Calc::spend_max (наибольшее списание плана, не больше бюджета сделки; решение 66).
//! Решение 166 (Тигран, 05.11.2026): «механизм должен быть в ядре для всех бирж». Каждая сделка в предпроверке
//! спрашивает у биржи A (контракт withdraw_hold с котируемой монетой): MEXC, Binance, OKX — удержания депозитов; Gate —
//! депозиты DEP_CREDITED; KuCoin — lockedAmount квоты вывода; Bybit — доступно минус «можно вывести». Биржа без
//! официального источника (BingX, Bitget) отвечает «неизвестно» — проверка не блокирует, одна запись в журнал на
//! площадку (core.precheck_withdraw); дальше страхует повтор вывода (core/transfer_locked.rs).
//! Решение 266 (precheck.parallel = да): check разделён на fetch («получить» — запросы к бирже, от расчёта не зависят,
//! идут одновременно с запросами сторон, precheck_par.rs) и settle («решить» — на прежнем месте шага, после расчёта и
//! проверки продажи); check — прежний, для выключенного режима. Часть 4 (precheck.balance_first = да): свободный баланс
//! котируемой A уже прочитан предпроверкой тем же запросом (limits::free_now той же биржи и монеты; счёт DEX бирже не
//! нужен — gateway/registry.rs get_as) — fetch его не читает, settle берёт прочитанный: один запрос баланса на сделку;
//! если тот не получен — settle читает сам, как check.
//! Кто вызывает: core/precheck.rs (после пересчёта с финальной котировкой), core/precheck_par.rs (fetch).
//! От чего зависит: trade_ops.rs (вид площадки), limits.rs (free_now), convert_at.rs (price_at), deps.rs (шлюз).
//! Настройки: venue.quote_asset (по площадке A); новых нет.

use std::collections::HashSet;
use std::sync::Mutex;

use rust_decimal::Decimal;

use crate::core::types::{Direction, VenueId, WithdrawHold};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::costs::Calc;
use super::engine::{Core, rd};

/// Что делает: проверка «можно вывести» на бирже A перед покупкой. Принимает: ядро, направление, свежий расчёт.
/// Возвращает: Ok — A не биржа, удержаний нет или выводимого хватает. Ошибки: текст причины — сбой запроса удержаний,
/// нет цены удержания, не задана venue.quote_asset, сбой баланса, выводимого меньше суммы покупки.
pub async fn check(core: &Core, d: &Direction, plan: &Calc) -> Result<(), String> {
    let a = &d.buy_on;
    if super::trade_ops::kind(core, a)? != VenueKind::Cex {
        return Ok(());
    }
    let at = At {
        venue: Some(&a.0),
        coin: None,
    };
    let quote = core
        .inner
        .deps
        .settings
        .text("venue.quote_asset", at)
        .map_err(|e| e.message)?;
    let (held, why) = match held(core, a, &quote).await? {
        Held::Nothing => return Ok(()),
        Held::Unknown(why) => {
            unknown_once(core, a, &why).await;
            return Ok(());
        }
        Held::Usdt { usdt, why } => (usdt, why),
    };
    let free = super::limits::free_now(core, a, None, &quote).await?;
    verdict(&a.0, &quote, free, held, plan.spend_max, &why)
}

/// Решение 266 (precheck.parallel): всё, что проверка «можно вывести» берёт у биржи A, — без расчёта сделки.
pub enum Got {
    /// Удержаний нет.
    Nothing,
    /// Биржа не может сказать — причина от адаптера (запись в журнал — в settle, на прежнем месте шага).
    Unknown(String),
    /// Удержания есть: котируемая монета A, свободно на A (None — fetch баланс не читал), удержано в USDT, тексты
    /// удержаний.
    Held {
        quote: String,
        free: Option<Decimal>,
        usdt: Decimal,
        why: String,
    },
}

/// Что делает: решение 266 — «получить» из check: те же запросы в том же порядке (venue.quote_asset, удержания, при
/// непустых удержаниях — свободный баланс котируемой), от расчёта сделки не зависят — предпроверка запускает их
/// одновременно с запросами сторон (precheck_par.rs). Принимает: ядро, биржу A (вид Cex проверяет вызывающий), читать
/// ли свободный баланс (false — часть 4: его читает предпроверка, precheck_balance.rs).
/// Возвращает: Got. Ошибки: те же тексты, что у check (не задана venue.quote_asset, сбой удержаний, нет цены
/// удержания, сбой баланса).
pub async fn fetch(core: &Core, a: &VenueId, own_free: bool) -> Result<Got, String> {
    let at = At {
        venue: Some(&a.0),
        coin: None,
    };
    let quote = core
        .inner
        .deps
        .settings
        .text("venue.quote_asset", at)
        .map_err(|e| e.message)?;
    let (usdt, why) = match held(core, a, &quote).await? {
        Held::Nothing => return Ok(Got::Nothing),
        Held::Unknown(why) => return Ok(Got::Unknown(why)),
        Held::Usdt { usdt, why } => (usdt, why),
    };
    let free = if own_free {
        Some(super::limits::free_now(core, a, None, &quote).await?)
    } else {
        None
    };
    Ok(Got::Held {
        quote,
        free,
        usdt,
        why,
    })
}

/// Что делает: решение 266 — «решить» из check по уже полученному (fetch) и свежему расчёту: запись «неизвестно» в
/// журнал (одна на площадку, как у check) и решение по числам (verdict). Принимает: ядро, биржу A, полученное,
/// расчёт, свободный баланс котируемой A, прочитанный предпроверкой (часть 4; None — нет). Свободного нет ни в
/// полученном, ни от предпроверки — читается здесь, как в check. Возвращает: Ok — удержаний нет, неизвестно или
/// выводимого хватает. Ошибки: текст verdict; сбой баланса.
pub async fn settle(
    core: &Core,
    a: &VenueId,
    got: Got,
    plan: &Calc,
    known: Option<Decimal>,
) -> Result<(), String> {
    match got {
        Got::Nothing => Ok(()),
        Got::Unknown(why) => {
            unknown_once(core, a, &why).await;
            Ok(())
        }
        Got::Held {
            quote,
            free,
            usdt,
            why,
        } => {
            let free = match free.or(known) {
                Some(f) => f,
                None => super::limits::free_now(core, a, None, &quote).await?,
            };
            verdict(&a.0, &quote, free, usdt, plan.spend_max, &why)
        }
    }
}

/// Удержания вывода биржи (решение 166).
pub enum Held {
    /// Удержаний нет.
    Nothing,
    /// Биржа не может сказать (нет в документации) — причина от адаптера.
    Unknown(String),
    /// Удержано на сумму usdt (в USDT), тексты удержаний через «; ».
    Usdt { usdt: Decimal, why: String },
}

/// Сколько удержано от вывода на бирже A — контракт withdraw_hold с котируемой монетой, в USDT по живым ценам.
/// Тот же расчёт — у ручного перевода USDT (решение 176, usdt_transfer_plan.rs). Принимает: ядро, биржу, котируемую.
/// Возвращает: Held. Ошибки: сбой запроса удержаний («не торгуем вслепую»), нет цены удержания.
pub async fn held(core: &Core, a: &VenueId, quote: &str) -> Result<Held, String> {
    let gw = core.inner.deps.gw.get(a).await.map_err(|e| e.message)?;
    let hold = gw.withdraw_hold(quote).await.map_err(|e| {
        format!(
            "{}: сколько можно вывести — не получено: {} — не торгуем вслепую",
            a.0, e.message
        )
    })?;
    if let Some(why) = &hold.unknown {
        return Ok(Held::Unknown(why.clone()));
    }
    if hold.is_empty() {
        return Ok(Held::Nothing);
    }
    let usdt = held_usdt(core, a, quote, &hold).await?;
    Ok(Held::Usdt {
        usdt,
        why: reasons(&hold),
    })
}

/// Что делает: стоимость всех удержаний в USDT. Котируемая монета A — как есть; другая — по живому стакану биржи
/// (convert_at::price_at — как у оценки монет в USDT, credit_event.rs): сначала A, затем остальные биржи по их тикеру
/// того же адреса (решение 211, ревью В7).
/// Ошибки: текст — у удержания нет живой цены (неизвестная стоимость = сделки нет).
async fn held_usdt(
    core: &Core,
    a: &VenueId,
    quote: &str,
    hold: &WithdrawHold,
) -> Result<Decimal, String> {
    let mut cex = vec![a.clone()];
    cex.extend(
        rd(&core.inner.kinds)
            .iter()
            .filter(|(v, k)| *k == VenueKind::Cex && v != a)
            .map(|(v, _)| v.clone()),
    );
    let mut sum = Decimal::ZERO;
    for h in &hold.items {
        if h.asset == quote {
            sum += h.amount;
            continue;
        }
        // Решение 211 (ревью В7): удержание — в тикере биржи A; на других биржах — по их тикеру того же адреса.
        let p = super::convert_at::price_at(core, &cex, a, &h.asset, None, h.amount)
            .await
            .map_err(|e| {
                format!(
                    "{}: {} — стоимость в USDT неизвестна ({e}), вывод может не пройти",
                    a.0, h.reason
                )
            })?;
        sum += h.amount * p;
    }
    Ok(sum)
}

/// Площадки, о которых уже записано «неизвестно» (одна запись на площадку за запуск).
static UNKNOWN_LOGGED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Решение 166: биржа не может сказать, сколько можно вывести, — одна запись в журнал на площадку, сделка не
/// блокируется. Принимает: ядро, площадку, причину от адаптера. Ошибок нет.
async fn unknown_once(core: &Core, a: &VenueId, why: &str) {
    let fresh = {
        let mut g = UNKNOWN_LOGGED.lock().unwrap_or_else(|p| p.into_inner());
        g.get_or_insert_with(HashSet::new).insert(a.0.clone())
    };
    if !fresh {
        return;
    }
    let text = format!(
        "{}: сколько можно вывести до покупки — неизвестно ({why}); проверка не блокирует, страхует повтор вывода \
         (transfer.locked_retry_ms, transfer.locked_retries_max)",
        a.0
    );
    core.inner
        .deps
        .log(
            Level::Info,
            "core.precheck_withdraw",
            Some(&a.0),
            text,
            None,
        )
        .await;
}

/// Тексты удержаний через «; ». Ошибок нет.
fn reasons(hold: &WithdrawHold) -> String {
    let parts: Vec<&str> = hold.items.iter().map(|h| h.reason.as_str()).collect();
    parts.join("; ")
}

/// Что делает: решение по числам (чистая функция). Принимает: площадку, котируемую монету, свободно на A, удержания в
/// USDT, сумму покупки, тексты удержаний. Возвращает: Ok — удержаний нет (held ≤ 0) или «можно вывести» не меньше
/// покупки. Ошибки: текст «mexc: предварительно зачислено … — вывод не пройдёт: можно вывести …».
pub fn verdict(
    venue: &str,
    quote: &str,
    free: Decimal,
    held: Decimal,
    spend: Decimal,
    why: &str,
) -> Result<(), String> {
    if held <= Decimal::ZERO {
        return Ok(());
    }
    let can = free - held;
    if can >= spend {
        return Ok(());
    }
    Err(format!(
        "{venue}: {why} — вывод не пройдёт: можно вывести {free} − {} = {} {quote} меньше покупки {} {quote}",
        held.round_dp(4),
        can.round_dp(4),
        spend.round_dp(4)
    ))
}
