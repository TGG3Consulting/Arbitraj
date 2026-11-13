//! Файл: core/precheck_par.rs
//! Что делает: решение 266 (Тигран 13.11.2026), часть 2 — независимые запросы предпроверки одновременно
//! (precheck.parallel = да). После стоп-условий одним tokio::join! в ТОЙ ЖЕ задаче сделки (не spawn: срочность Trade и
//! замеры задачи — task_local, новая задача их не наследует) идут:
//! (а) сторона A: биржа — справочник сетей (статус вывода); DEX — баланс BNB;
//! (б) сторона B: биржа — справочник сетей (статус ввода) → сторона пары → адрес депозита, строго по очереди (адрес
//!     депозита у части бирж создаётся этим запросом — KuCoin, Kraken; он остаётся после успеха «сетей B» и пары);
//!     DEX — баланс BNB;
//! (в) A — биржа: удержания вывода (precheck_withdraw.rs fetch — то, что не зависит от расчёта);
//! (г) часть 4 (precheck.balance_first = да): свободный баланс котируемой A (precheck_balance.rs read) — его итог
//!     разбирает precheck.rs на месте шага «свободный баланс» (после шагов сторон, перед расчётом); удержания (в) при
//!     этом свой баланс не читают — один запрос баланса на сделку.
//! Часть 3 (precheck.networks_memory = да): справочник сетей стороны — из памяти, пока она свежая (precheck_nets.rs);
//! источник каждого шага сетей возвращается для подробного замера.
//! Итоги разбираются СТРОГО в порядке последовательного кода (precheck.rs steps): A — сети → сторона пары A; B — вид
//! площадки → сети → сторона пары → адрес; ошибка BNB — на месте своей стороны. Возвращается первая по порядку ошибка
//! с тем же текстом; уведомление risk.low_bnb — только если ошибка BNB первая по порядку (не больше одного). Итог (в)
//! не разбирается здесь: его ошибка — на прежнем месте шага «удержания вывода» (после расчёта и проверки продажи).
//! Время шагов — те же имена в прежнем порядке, до первой ошибки включительно; каждый шаг меряет себя сам.
//! Отличие от последовательного кода: запросы стороны B и удержаний уходят, даже если сторона A откажет.
//! Кто вызывает: core/precheck.rs (steps, только при precheck.parallel = да).
//! От чего зависит: core/precheck.rs (transfer_open, pair_side, bnb_ok), precheck_deposit.rs, precheck_withdraw.rs,
//! trade_ops.rs (вид площадки).
//! Настройки: через вызываемые шаги (включатели решения 266 читает executor.rs, precheck_fast.rs).

use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

use rust_decimal::Decimal;

use crate::core::types::{DexAccountId, Direction};
use crate::gateway::contract::VenueKind;

use super::engine::Core;
use super::precheck::{bnb_ok, pair_side, transfer_open};
use super::precheck_balance::Read;
use super::precheck_nets::Src;
use super::precheck_withdraw::Got;

/// Шаг с временем: (имя, мс).
pub type Mark = (String, u128);

/// Удержания вывода A, полученные заранее: итог запроса и его время, мс.
pub type Held = (Result<Got, String>, u128);

/// Отказ стороны: обычный или «мало BNB» (с уведомлением risk.low_bnb, как в последовательном коде).
enum Stop {
    Plain(String),
    LowBnb(String),
}

/// Свободный баланс A, прочитанный заранее (часть 4): итог чтения и его время, мс.
pub type Free = (Result<Read, String>, u128);

/// Источник справочника сетей шага (часть 3): (имя шага, текст источника).
pub type Source = (String, String);

/// Итог sides: время шагов, источники справочников сетей, удержания вывода A и свободный баланс A (или первая по
/// порядку причина отказа сторон).
pub type Out = (
    Vec<Mark>,
    Vec<Source>,
    Result<(Option<Held>, Option<Free>), String>,
);

/// Режим остальных частей решения 266 для одновременных шагов.
#[derive(Clone, Copy)]
pub struct Par<'a> {
    /// precheck.networks_memory = да.
    pub nets: bool,
    /// precheck.balance_first = да.
    pub balance: bool,
    /// Счёт DEX сделки (баланс кошелька DEX A).
    pub acc: Option<&'a DexAccountId>,
}

/// Что делает: run с будущим в куче — оно собирается здесь, а не в кадре precheck.rs steps (кадр шагов предпроверки не
/// растёт, когда режим выключен). Принимает, возвращает: как run.
#[inline(never)]
pub(super) fn sides<'a>(
    core: &'a Core,
    d: &'a Direction,
    par: Par<'a>,
) -> Pin<Box<dyn Future<Output = Out> + Send + 'a>> {
    Box::pin(run(core, d, par))
}

/// Что делает: шаги сторон A и B и запрос удержаний вывода — одновременно; разбор — в прежнем порядке шагов.
/// Принимает: ядро, направление, режим частей 3 и 4. Возвращает: (время шагов в прежнем порядке до первой ошибки
/// включительно; источники справочников сетей (часть 3); Ok — удержания вывода A, если A биржа (итог запроса и время),
/// иначе None, и свободный баланс A (часть 4; итог чтения и время), иначе None). Ошибки: текст первой по порядку
/// причины отказа сторон — тот же, что у последовательного кода.
async fn run(core: &Core, d: &Direction, par: Par<'_>) -> Out {
    let (a, b) = (&d.buy_on, &d.sell_on);
    let mut marks = Vec::new();
    let mut srcs: Vec<Source> = Vec::new();
    // Вид A — первым, как в последовательном коде: ошибка — сразу, запросов нет.
    let ka = match super::trade_ops::kind(core, a) {
        Ok(k) => k,
        Err(e) => return (marks, srcs, Err(e)),
    };
    // Вид B: его ошибка встаёт на место стороны B (после шагов A) — запросов стороны B при ней нет.
    let kb = super::trade_ops::kind(core, b);
    let side_a = async {
        let at = Instant::now();
        match ka {
            VenueKind::Cex => {
                let mut src = Src::Http;
                let r = transfer_open(core, a, d, true, par.nets.then_some(&mut src)).await;
                (
                    format!("справочник сетей {}", a.0),
                    at.elapsed().as_millis(),
                    r,
                    par.nets.then(|| src.text()),
                )
            }
            VenueKind::Dex => {
                let r = bnb_ok(core, d, a, Decimal::ZERO).await;
                (
                    format!("баланс BNB {}", a.0),
                    at.elapsed().as_millis(),
                    r,
                    None,
                )
            }
        }
    };
    let side_b = async {
        let mut m: Vec<Mark> = Vec::new();
        let mut from: Option<Source> = None;
        let r = async {
            match kb {
                Err(e) => Err(Stop::Plain(e)),
                Ok(VenueKind::Cex) => {
                    let at = Instant::now();
                    let mut src = Src::Http;
                    let r = transfer_open(core, b, d, false, par.nets.then_some(&mut src)).await;
                    let step = format!("справочник сетей {}", b.0);
                    if par.nets {
                        from = Some((step.clone(), src.text()));
                    }
                    m.push((step, at.elapsed().as_millis()));
                    r.map_err(Stop::Plain)?;
                    pair_side(core, b, d, false).map_err(Stop::Plain)?;
                    // Решение 118 (Я-5): адрес депозита — только после успеха сетей и пары B (как раньше).
                    let at = Instant::now();
                    let r = super::precheck_deposit::ready(core, d).await;
                    m.push((format!("адрес депозита {}", b.0), at.elapsed().as_millis()));
                    r.map_err(Stop::Plain)
                }
                Ok(VenueKind::Dex) => {
                    let at = Instant::now();
                    let r = bnb_ok(core, d, b, Decimal::ZERO).await;
                    m.push((format!("баланс BNB {}", b.0), at.elapsed().as_millis()));
                    r.map_err(Stop::LowBnb)
                }
            }
        }
        .await;
        (m, from, r)
    };
    let held = async {
        if ka != VenueKind::Cex {
            return None;
        }
        let at = Instant::now();
        // Часть 4: свободный баланс читает (г) — удержания свой баланс не читают (один запрос на сделку).
        let r = super::precheck_withdraw::fetch(core, a, !par.balance).await;
        Some((r, at.elapsed().as_millis()))
    };
    let free = async {
        if !par.balance {
            return None;
        }
        let at = Instant::now();
        let r = super::precheck_balance::read(core, d, par.acc).await;
        Some((r, at.elapsed().as_millis()))
    };
    let ((name_a, ms_a, ra, src_a), (marks_b, src_b, rb), held, free) =
        tokio::join!(side_a, side_b, held, free);
    // Разбор — в порядке последовательного кода. Сторона A.
    srcs.extend(src_a.map(|s| (name_a.clone(), s)));
    marks.push((name_a, ms_a));
    if let Err(e) = ra {
        if ka == VenueKind::Dex {
            core.inner.deps.notify("risk.low_bnb", Some(&a.0), &e).await;
        }
        return (marks, srcs, Err(e));
    }
    if ka == VenueKind::Cex
        && let Err(e) = pair_side(core, a, d, true)
    {
        return (marks, srcs, Err(e));
    }
    // Сторона B.
    marks.extend(marks_b);
    srcs.extend(src_b);
    match rb {
        Ok(()) => (marks, srcs, Ok((held, free))),
        Err(Stop::Plain(e)) => (marks, srcs, Err(e)),
        Err(Stop::LowBnb(e)) => {
            core.inner.deps.notify("risk.low_bnb", Some(&b.0), &e).await;
            (marks, srcs, Err(e))
        }
    }
}
