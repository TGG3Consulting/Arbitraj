//! Файл: core/cycle_watch.rs
//! Что делает: исполнение направления DEX ↔ DEX (решение 124; направление — 85, гейт — 86). Одна задача на зарегистрированную площадку:
//! ждёт толчка (новые состояния пулов — tick.rs; толчки во время поиска сливаются в один повтор), для своей площадки
//! DEX с dex_dex.execute = да берёт сигналы циклов (Venue::dex_cycles — тот же граф пулов решения 80, оба обхода цикла —
//! отдельные сигналы, считаются независимо; вход сигнала — оптимальный в пределах dex_dex.probe_input_base), отбирает
//! кандидатов (исполним как есть, чистая прибыль > 0; лучший первым; порог dex_dex.min_profit_usd — в плане) и по
//! первому годному: предпроверка risk::can_open (money.*, risk.*, режим
//! системы, пауза площадки, карантин) → план (cycle_plan.rs: бюджет, размер, порог, граница) →
//! - сухой режим (dry.enabled, решение 74): сухая сделка цикла (Venue::dex_dry_cycle: сборка и eth_simulateV1, ничего
//!   не уходит) → журнал core.cycle_dry и уведомление cycle.dry — один раз на смену итога по циклу;
//! - живой: сделка (cycle_deal.rs). Не больше одного цикла за проход: очередь nonce кошелька общая (решение 45),
//!   циклы идут по одному.
//!
//! Решение 404: итог каждого прохода — журнал core.cycle_scan свёрнуто (cycle_scan_log.rs): сколько циклов просмотрено и
//! с курсом круга > 1, где отобранные отпали (адаптер), сколько сигналов неисполнимы / без прибыли (ядро).
//! Причины отказа — в журнал core.cycle один раз на смену причины по циклу (cycle_state.rs), а не на каждом блоке;
//! причины на всю площадку (поиск циклов не удался, нет кошелька «DEX ↔ DEX» в маппинге) — один раз на площадку (решение 139).
//! dex_dex.scan_min_interval_ms (необязательная) — наименьший промежуток между проходами (как
//! signal.eval_min_interval_ms, решение 109); не задана — без предела (проход сразу за толчком).
//! Кто вызывает: core/engine.rs (задача run).
//! От чего зависит: core/{cycle_plan, cycle_deal, cycle_state, cycle_scan_log, risk, dry, deps, dex_account}.rs, gateway (Venue,
//! VenueKind).
//! Настройки: venue.enabled (решение 489: выключенная DEX циклов не исполняет), dex_dex.execute, dex_dex.min_profit_usd,
//! dex_dex.scan_min_interval_ms [площадка DEX]; через вызываемые —
//! money.*, risk.*, size.*, dex_dex.size_step_base, dry.*.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use rust_decimal::Decimal;

use crate::core::types::{CycleTradeRequest, DexCycle, VenueId};
use crate::settings::read::At;
use crate::storage::journal::Level;
use std::future::Future;

use super::cycle_deal::{self, Outcome};
use super::cycle_plan::{self, CyclePlan};
use super::cycle_state::{key, venue_key};
use super::engine::Core;

/// Пул будит только свою площадку. Один проход и один объединённый повтор на площадку.
pub async fn run(core: Core, venue: VenueId) {
    let Some(lane) = core.inner.cycles.lane(&venue) else {
        return;
    };
    passes(lane, || async {
        if !enabled(&core, &venue) {
            return None;
        }
        let gap = core
            .inner
            .deps
            .settings
            .integer(
                "dex_dex.scan_min_interval_ms",
                At {
                    venue: Some(&venue.0),
                    coin: None,
                },
            )
            .ok()
            .map(Duration::from_millis);
        pass(&core, &venue).await;
        gap
    })
    .await;
}

pub(super) fn enabled(core: &Core, v: &VenueId) -> bool {
    super::registry_sync::enabled(core, v)
        && core
            .inner
            .deps
            .settings
            .flag(
                "dex_dex.execute",
                At {
                    venue: Some(&v.0),
                    coin: None,
                },
            )
            .unwrap_or(false)
}

async fn passes<F: Future<Output = Option<Duration>>>(
    wake: &tokio::sync::Notify,
    mut pass: impl FnMut() -> F,
) {
    loop {
        wake.notified().await;
        let started = Instant::now();
        if let Some(left) = pass()
            .await
            .and_then(|gap| gap.checked_sub(started.elapsed()))
        {
            tokio::time::sleep(left).await;
        }
    }
}

/// Один проход по площадке: сигналы → кандидаты → первый годный в сделку (или сухую сделку). Ошибок нет.
async fn pass(core: &Core, v: &VenueId) {
    let deps = &core.inner.deps;
    let scan_at = Instant::now();
    let (list, tally) = match deps.gw.get(v).await {
        Ok(x) => (x.dex_cycles().await, x.dex_cycle_tally()),
        Err(e) => (Err(e), Vec::new()),
    };
    let scan_ms = scan_at.elapsed().as_millis() as u64;
    let list = match list {
        Ok(l) => l,
        Err(e) => {
            let why = format!("поиск циклов: {}", e.message);
            return say(core, v, &venue_key(v, "scan"), &why).await;
        }
    };
    core.inner.cycles.fresh(&venue_key(v, "scan"), "");
    // Решение 404: итог прохода (адаптер — по базовым, ядро — отбор кандидатов) в журнал core.cycle_scan, свёрнуто.
    super::cycle_scan_log::note(core, v, tally, &list, scan_ms).await;
    let cands: Vec<DexCycle> = list
        .into_iter()
        .filter(|c| {
            c.venue == *v && c.executable && c.net_profit.is_some_and(|p| p > Decimal::ZERO)
        })
        .collect();
    let alive: HashSet<String> = cands.iter().map(key).collect();
    core.inner.cycles.retain(&v.0, &alive);
    // Решение 139: причина на всю площадку (нет кошелька «DEX ↔ DEX» в маппинге) — одна запись на смену причины, а не
    // по записи на каждый цикл каждого блока (ключи циклов меняются вместе с сигналами).
    if !cands.is_empty() {
        let wk = venue_key(v, "wallet");
        if deps
            .settings
            .accounts()
            .row(crate::settings::accounts::ROW_DEX_DEX)
            .is_none()
        {
            return say(core, v, &wk, super::dex_account::NO_WALLET).await;
        }
        core.inner.cycles.fresh(&wk, "");
    }
    for c in cands {
        let k = key(&c);
        let d = match cycle_plan::direction(core, &c) {
            Ok(d) => d,
            Err(why) => return say(core, v, &k, &why).await,
        };
        if let Err(why) = super::risk::can_open(core, &d) {
            return say(core, v, &k, &why).await;
        }
        let plan = match cycle_plan::plan(core, &c, None).await {
            Ok(p) => p,
            Err(why) => {
                say(core, v, &k, &why).await;
                continue;
            }
        };
        if super::dry::mode(&deps.settings).is_dry() {
            return dry(core, &k, plan).await;
        }
        match cycle_deal::run(core, plan).await {
            Outcome::Done => return,
            Outcome::Busy => {
                // Решение 404: раньше молча. Причина — один раз на смену (как прочие отказы цикла).
                let why = "по базовой монете уже идёт сделка (замок «одна сделка на монету»)";
                say(core, v, &k, why).await;
                continue;
            }
            Outcome::Refused(why) => say(core, v, &k, &why).await,
        }
    }
}

/// Сухой режим: сухая сделка цикла и итог — в журнал и cycle.dry (один раз на смену итога). Ошибок нет.
async fn dry(core: &Core, k: &str, plan: CyclePlan) {
    let deps = &core.inner.deps;
    let v = plan.cycle.venue.clone();
    let req = request(&plan, "dry-cycle");
    let r = match deps.gw.get_as(&v, Some(&plan.account)).await {
        Ok(x) => x.dex_dry_cycle(req, false).await,
        Err(e) => Err(e),
    };
    let text = match &r {
        Ok(x) if x.ok => format!(
            "Сухой режим: цикл {} прошёл бы — вход {}, граница {}, по симуляции {} ({}), чистая ≈ {} USDT",
            plan.cycle.base_ticker,
            plan.input,
            plan.min_out,
            x.sim_out.map(|o| o.to_string()).unwrap_or_default(),
            x.form.clone().unwrap_or_default(),
            plan.profit_usd.round_dp(4)
        ),
        Ok(x) => format!(
            "Сухой режим: цикл {} не прошёл бы — {}",
            plan.cycle.base_ticker,
            x.reason.clone().unwrap_or_default()
        ),
        Err(e) => format!(
            "Сухой режим: сухая сделка цикла не выполнена — {}",
            e.message
        ),
    };
    // Память причины — по самому итогу (без чисел входа и симуляции, они меняются на каждом блоке).
    let tag = match &r {
        Ok(x) if x.ok => "ok".to_string(),
        Ok(x) => x.reason.clone().unwrap_or_default(),
        Err(e) => e.message.clone(),
    };
    if core.inner.cycles.fresh(k, &tag) {
        let data = r.ok().and_then(|x| serde_json::to_value(x).ok());
        deps.log(Level::Info, "core.cycle_dry", Some(&v.0), &text, data)
            .await;
        deps.notify("cycle.dry", Some(&v.0), &text).await;
    }
}

/// Запрос сделки цикла из плана. Ошибок нет.
pub fn request(plan: &CyclePlan, deal_id: &str) -> CycleTradeRequest {
    CycleTradeRequest {
        venue: plan.cycle.venue.clone(),
        deal_id: deal_id.to_string(),
        cycle: plan.cycle.clone(),
        input: plan.input,
        min_out: plan.min_out,
    }
}

/// Причина отказа по циклу — в журнал core.cycle, только при смене причины. Ошибок нет.
async fn say(core: &Core, v: &VenueId, k: &str, why: &str) {
    if core.inner.cycles.fresh(k, why) {
        let text = format!("Цикл DEX ↔ DEX на {}: сделки нет — {why}", v.0);
        core.inner
            .deps
            .log(Level::Info, "core.cycle", Some(&v.0), text, None)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cycle_state::CycleState;
    use crate::gateway::contract::VenueKind;
    use futures_util::FutureExt;
    use std::cell::Cell;
    use tokio::sync::Notify;

    #[tokio::test]
    async fn other_chain_processes_blocks_while_peer_is_waiting_and_wakes_once() {
        let base = VenueId("base_dex".into());
        let eth = VenueId("eth_dex".into());
        for (slow, fast) in [(&base, &eth), (&eth, &base)] {
            let lanes = CycleState::registered(&[
                (slow.clone(), VenueKind::Dex),
                (fast.clone(), VenueKind::Dex),
            ]);
            let release = Notify::new();
            let blocked = Cell::new(0);
            let healthy = Cell::new(0);
            let mut slow_pass = std::pin::pin!(passes(lanes.lane(slow).unwrap(), || async {
                blocked.set(blocked.get() + 1);
                release.notified().await;
                None
            }));
            let mut fast_pass = std::pin::pin!(passes(lanes.lane(fast).unwrap(), || async {
                healthy.set(healthy.get() + 1);
                None
            }));
            lanes.changed(slow);
            assert!(slow_pass.as_mut().now_or_never().is_none());
            assert!(fast_pass.as_mut().now_or_never().is_none());
            assert_eq!((blocked.get(), healthy.get()), (1, 0));
            for _ in 0..8 {
                lanes.changed(slow);
            }
            for expected in 1..=3 {
                lanes.changed(fast);
                assert!(fast_pass.as_mut().now_or_never().is_none());
                assert_eq!((blocked.get(), healthy.get()), (1, expected));
            }
            release.notify_one();
            assert!(slow_pass.as_mut().now_or_never().is_none());
            release.notify_one();
            assert!(slow_pass.as_mut().now_or_never().is_none());
            assert_eq!((blocked.get(), healthy.get()), (2, 3));
        }
    }
}
