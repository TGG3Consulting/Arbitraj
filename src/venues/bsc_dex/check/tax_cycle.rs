//! Файл: venues/bsc_dex/check/tax_cycle.rs
//! Что делает: цикл проверки налогов монет (решение 30). До проверки монета «не проверена» (CoinCheck NotChecked —
//! торговли нет, но и не чёрный список). Монета проверяется, как только прочитаны ЕЁ пулы (pools/loading.rs, решение 61):
//! чужие недочитанные пулы её не держат. Сводка первого прохода — когда у всех монет, кроме тех, чьи пулы застряли (ждут
//! незаданную настройку), была первая проверка (tax_wait.rs; «пулы не читаются: N» — отдельным числом): одна
//! запись в журнал и событие CheckPass ядру (одно сводное уведомление, core/first_pass.rs).
//! Дальше — полный проход раз в tax.recheck_interval_s. Между проходами:
//! - «Перепроверить» — только переданные монеты (check/recheck.rs);
//! - «пул готов» — ждущие монеты, у которых появился достоверный пул или дочитались свои пулы;
//! - ждущие из-за сбоя узла (429, нет связи — не вердикт) — повтор через tax.retry_s.
//!
//! Все запросы к узлу — в общем темпе node/pace.rs (bsc.node_http_rps): проход идёт ровно, а не залпом.
//! Кто вызывает: pools/market.rs (запуск при подписке).
//! От чего зависит: check/tax_pass.rs, check/taxes.rs, check/recheck.rs, pools/listen.rs (Pools), pools/progress.rs.
//! Настройки: tax.recheck_interval_s, tax.retry_s (вся система), check.probe_usdt [bsc_dex].

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use alloy_primitives::Address;

use crate::core::types::{Asset, CheckState, CoinCheck};
use crate::gateway::events::MarketEvent;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::super::pools::listen::Pools;
use super::recheck::Queue;
use super::tax_deep::has_deep;
use super::tax_pass::{self, Pass, Why};
use super::tax_wait::{self, First};
use super::taxes;

/// Цикл налогов. Принимает: окружение, монеты (адрес → тикер), USDT, очередь «Перепроверить». Работает постоянно.
pub async fn taxes(p: Pools, coins: HashMap<Address, String>, usdt: Address, queue: Queue) {
    for (addr, ticker) in &coins {
        let c = CoinCheck {
            venue: p.ctx.venue(),
            asset: Asset {
                ticker: ticker.clone(),
                bsc_contract: Some(*addr),
            },
            taxes: CheckState::NotChecked,
            source: taxes::SOURCE.into(),
            checked_at: None,
            wait: Some("проверка ещё не начиналась: ждёт своих пулов".into()),
        };
        p.checks.set_coin(c.clone());
        let _ = p.out.try_send(MarketEvent::CoinCheck(c));
    }
    let all: Vec<(Address, String)> = coins.into_iter().collect();
    let mut seen = HashSet::new();
    // Сводка первого прохода — когда у всех монет, кроме ждущих своих пулов, была первая проверка (tax_wait.rs).
    let mut sum = First::new(all.iter().map(|(a, _)| *a));
    loop {
        let was_sent = sum.sent;
        let pass = tax_pass::check(&p, &all, usdt, &mut seen, &sum.left).await;
        settle(&p, &pass, &mut sum).await;
        // Полный проход после первого — своя строка журнала (первый — сводкой в settle).
        if was_sent {
            let msg = format!(
                "Проверка налогов: прошло {}, не прошло {}, ждут (нет пула, сбой узла) {}, не задана настройка {}, пулы ещё загружаются {}",
                pass.ok, pass.failed, pass.no_pool, pass.config, pass.loading
            );
            p.ctx.log(Level::Info, "dex.check", msg, None).await;
        }
        let Some(interval) = secs(&p, "tax.recheck_interval_s").await else {
            return;
        };
        let next = tokio::time::Instant::now() + interval;
        let mut waiting = pass.waiting;
        loop {
            let node_wait = waiting.iter().any(|(_, _, w)| *w == Why::Node);
            let retry = if node_wait {
                secs(&p, "tax.retry_s").await
            } else {
                None
            };
            // Свои пулы дочитались, пока шла прошлая проверка (сигнал мог прийти без ждущего) — сразу.
            let pending = |a: Address| p.loading.pending(&p, a);
            let now = tax_wait::due(&waiting, false, pending, |t| has_deep(&p, t));
            let list: Vec<(Address, String)> = if !now.is_empty() {
                now
            } else {
                tokio::select! {
                    _ = tokio::time::sleep_until(next) => break,
                    _ = queue.notified() => {
                        let (tickers, _) = queue.take();
                        all.iter().filter(|(_, t)| tickers.contains(t)).cloned().collect()
                    }
                    // «Пул готов»: свои пулы прочитаны или появился достоверный пул — проверка сразу (решение 63).
                    _ = p.signals.ready.notified() => tax_wait::due(&waiting, true, pending, |t| has_deep(&p, t)),
                    _ = sleep_opt(retry) => waiting
                        .iter()
                        .filter(|(_, _, w)| *w == Why::Node)
                        .map(|(a, t, _)| (*a, t.clone()))
                        .collect(),
                }
            };
            if list.is_empty() {
                continue;
            }
            let again = tax_pass::check(&p, &list, usdt, &mut seen, &sum.left).await;
            settle(&p, &again, &mut sum).await;
            waiting.retain(|(a, _, _)| !list.iter().any(|(r, _)| r == a));
            waiting.extend(again.waiting);
        }
    }
}

/// Пауза из настройки системы (секунды). None — не задана: запись в журнал (одна на причину). Ошибок нет.
async fn secs(p: &Pools, name: &str) -> Option<Duration> {
    match p.ctx.settings.integer(name, At::default()) {
        Ok(s) => Some(Duration::from_secs(s)),
        Err(e) => {
            let msg = format!("Цикл налогов: {}", e.message);
            p.signals.once(&p.ctx, Level::Error, "dex.check", msg).await;
            None
        }
    }
}

/// Сон на заданное время; None — никогда не просыпается. Ошибок нет.
async fn sleep_opt(d: Option<Duration>) {
    match d {
        Some(d) => tokio::time::sleep(d).await,
        None => std::future::pending().await,
    }
}

/// Учитывает проход в сводке первого прохода; пора — одна сводка в журнал и CheckPass ядру (core/first_pass.rs — одно
/// сводное уведомление). Ошибок нет.
async fn settle(p: &Pools, pass: &Pass, sum: &mut First) {
    let Some([ok, failed, waiting, not_configured, loading]) =
        sum.tally(pass, |a| p.loading.stuck_only(p, a))
    else {
        return;
    };
    let msg = format!(
        "Проверка налогов (первый проход): прошло {ok}, не прошло {failed}, ждут (нет пула, сбой узла) {waiting}, не задана настройка {not_configured}, пулы не читаются (ждут настройку) {loading}"
    );
    p.ctx.log(Level::Info, "dex.check", msg, None).await;
    let venue = p.ctx.venue();
    let ev = MarketEvent::CheckPass {
        venue,
        ok,
        failed,
        waiting,
        not_configured,
        loading,
    };
    let _ = p.out.send(ev).await;
}
