//! Файл: core/reconcile.rs
//! Что делает: сверка балансов (шаг 6 ТЗ) раз в ledger.reconcile_interval_s: балансы всех включённых
//! площадок через шлюз → экран «Балансы»; монеты, которые по учёту бота лежат на площадке (открытые
//! сделки не в пути), должны там быть — меньше — расхождение: новые сделки останавливаются (пауза)
//! и уведомление risk.reconcile_mismatch. Плюс: мало BNB на газ у DEX (risk.low_bnb), мало USDT
//! (risk.low_usdt по порогу alert.min_usdt площадки — low_usdt.rs, решение 54), возврат USDT «отдельно».
//! Кто вызывает: core/engine.rs (задача run).
//! От чего зависит: engine.rs, wait.rs, risk.rs, usdt_return.rs, precheck.rs (BNB), deal.rs.
//! Настройки: ledger.reconcile_interval_s, alert.min_usdt (через low_usdt.rs), risk.min_bnb_balance, venue.quote_asset.

use std::collections::{HashMap, HashSet};

use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::Notify;

use crate::core::types::VenueId;
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::Step;
use super::engine::{Core, lk, rd};
use super::risk::{self, Mode};
use super::view::CoreEvent;
use super::wait::{Pacer, Unit};

/// Задача сверки. Работает, пока жива система. Уведомление о каждой проблеме — один раз, пока она есть.
pub async fn run(core: Core) {
    let mut pacer = Pacer::new("ledger.reconcile_interval_s", Unit::Seconds, None);
    let quiet = Notify::new();
    let mut active: HashSet<String> = HashSet::new();
    loop {
        pacer.pause(&core.inner.deps, &quiet).await;
        let found = once(&core).await;
        for (code, venue, text) in &found {
            if active.insert(format!("{code}{text}")) {
                core.inner
                    .deps
                    .log(Level::Warn, "core.reconcile", venue.as_deref(), text, None)
                    .await;
                core.inner.deps.notify(code, venue.as_deref(), text).await;
            }
        }
        active.retain(|k| found.iter().any(|(c, _, t)| &format!("{c}{t}") == k));
    }
}

/// Один круг сверки. Возвращает: найденные проблемы (код события, площадка, текст).
async fn once(core: &Core) -> Vec<(&'static str, Option<String>, String)> {
    let deps = &core.inner.deps;
    let venues: Vec<(VenueId, VenueKind)> = rd(&core.inner.kinds)
        .iter()
        .filter(|(v, _)| super::registry_sync::enabled(core, v))
        .cloned()
        .collect();
    let mut out = vec![];
    let mut all = serde_json::Map::new();
    let mut free_usdt = HashMap::new();
    // Сколько монет бот считает своими на каждой площадке (сделки не в пути).
    let mut expected: HashMap<(VenueId, String), Decimal> = HashMap::new();
    for d in lk(&core.inner.deals)
        .values()
        .filter(|d| d.is_open() && d.step != Step::Transfer && d.step != Step::Buying)
    {
        *expected
            .entry((d.at.clone(), d.direction.asset.ticker.clone()))
            .or_default() += d.qty;
    }
    for (v, kind) in venues {
        let at = At {
            venue: Some(&v.0),
            coin: None,
        };
        let b = match deps.gw.get(&v).await {
            Ok(x) => x.balances().await,
            Err(e) => Err(e),
        };
        let b = match b {
            Ok(b) => b,
            Err(e) => {
                out.push((
                    "risk.reconcile_mismatch",
                    Some(v.0.clone()),
                    format!("{}: балансы не получены — {}", v.0, e.message),
                ));
                continue;
            }
        };
        all.insert(v.0.clone(), json!(b));
        if let Ok(q) = deps.settings.text("venue.quote_asset", at) {
            let f: Decimal = b.iter().filter(|x| x.asset == q).map(|x| x.free).sum();
            free_usdt.insert(v.clone(), f);
            // Решение 54: свой порог алерта по площадке, один раз на пересечение (low_usdt.rs).
            super::low_usdt::check(core, &v, f, &q).await;
        }
        if kind == VenueKind::Dex
            && let Err(e) = super::precheck::bnb_ok(core, &v, Decimal::ZERO).await
        {
            out.push(("risk.low_bnb", Some(v.0.clone()), e));
        }
        for ((ev, ticker), need) in expected.iter().filter(|((ev, _), _)| ev == &v) {
            let total: Decimal = b
                .iter()
                .filter(|x| &x.asset == ticker)
                .map(|x| x.free + x.locked)
                .sum();
            // BNB на DEX (решение 62): нативный + WBNB (адаптер отдаёт WBNB в locked строки BNB) минус резерв на газ —
            // резерв монетами сделки не считается; резерв не задан — сверить нельзя, это и есть причина.
            let have = match super::native::spendable(core, ev, ticker, total) {
                Ok(h) => h,
                Err(e) => {
                    out.push(("risk.reconcile_mismatch", Some(ev.0.clone()), e));
                    continue;
                }
            };
            if have < *need {
                out.push((
                    "risk.reconcile_mismatch",
                    Some(ev.0.clone()),
                    format!("{}: {ticker} по учёту {need}, на площадке {have}", ev.0),
                ));
            }
        }
    }
    let ex = lk(&core.inner.exposure).clone();
    core.inner.live.send(CoreEvent::Balances(json!({ "venues": all, "in_transit_usd": ex.in_transit, "per_coin_usd": ex.per_coin.iter().map(|(a, v)| (a.ticker.clone(), *v)).collect::<HashMap<_, _>>() })));
    if out.iter().any(|(c, _, _)| *c == "risk.reconcile_mismatch")
        && risk::snapshot(core).mode == Mode::Running
    {
        risk::set_mode(
            core,
            Mode::Paused,
            Some("расхождение при сверке балансов".into()),
        );
    }
    super::usdt_return::separate(core, &free_usdt).await;
    out
}
