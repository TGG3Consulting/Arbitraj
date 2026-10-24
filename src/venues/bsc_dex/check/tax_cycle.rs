//! Файл: venues/bsc_dex/check/tax_cycle.rs
//! Что делает: цикл проверки налогов монет (решение 30). До проверки монета «не проверена» (CoinCheck NotChecked —
//! торговли нет, но и не чёрный список). Первый проход — после стартовой загрузки пулов V2/V3 (pools/progress.rs);
//! дальше — полный проход раз в tax.recheck_interval_s или по кнопке «Перепроверить». Монеты, которым пока нечего
//! проверять (нет достоверного пула, нет источника пробы, узел не ответил) или не задана настройка, остаются «не
//! проверены» (прошлый итог не меняется) и проверяются снова, как только у них появится достоверный пул (сигнал
//! «пул готов»), — а не через час. Чёрный список — только если пробный обмен выполнен и откатился (check/taxes.rs).
//! Ошибка настройки — одна запись в журнал и одно событие dex.check_not_configured на каждую разную причину.
//! Кто вызывает: pools/market.rs (запуск при подписке), mod.rs (recheck_coins будит).
//! От чего зависит: check/taxes.rs, check/state.rs, check/cycle.rs (sleep), pools/listen.rs (Pools), pools/progress.rs.
//! Настройки: tax.recheck_interval_s (вся система), check.probe_usdt [bsc_dex].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use alloy_primitives::Address;
use tokio::sync::Notify;

use crate::core::types::{Asset, CheckState, CoinCheck};
use crate::gateway::events::MarketEvent;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::super::pools::listen::Pools;
use super::taxes::{self, Outcome};

/// Монеты, ждущие пула или настройки, и сколько каких итогов было в проходе.
#[derive(Default)]
struct Pass {
    /// Ждущие монеты и был ли у них достоверный пул при проверке.
    waiting: Vec<(Address, String, bool)>,
    ok: usize,
    failed: usize,
    no_pool: usize,
    config: usize,
}

/// Цикл налогов. Принимает: окружение, монеты (адрес → тикер), USDT, сигнал «перепроверить». Работает постоянно.
pub async fn taxes(p: Pools, coins: HashMap<Address, String>, usdt: Address, wake: Arc<Notify>) {
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
        };
        p.checks.set_coin(c.clone());
        let _ = p.out.try_send(MarketEvent::CoinCheck(c));
    }
    p.signals.wait_start().await;
    let all: Vec<(Address, String)> = coins.into_iter().collect();
    let mut config_seen = HashSet::new();
    loop {
        let pass = check(&p, &all, usdt, &mut config_seen).await;
        let msg = format!(
            "Проверка налогов: прошло {}, не прошло {}, ждут достоверного пула {}, не задана настройка {}",
            pass.ok, pass.failed, pass.no_pool, pass.config
        );
        p.ctx.log(Level::Info, "dex.check", msg, None).await;
        // Итог прохода — ядру: после первого прохода оно шлёт одно сводное уведомление (core/first_pass.rs).
        let _ = p.out.try_send(MarketEvent::CheckPass {
            venue: p.ctx.venue(),
            ok: pass.ok,
            failed: pass.failed,
            waiting: pass.no_pool,
            not_configured: pass.config,
        });
        let mut waiting = pass.waiting;
        let interval = match p
            .ctx
            .settings
            .integer("tax.recheck_interval_s", At::default())
        {
            Ok(s) => Duration::from_secs(s),
            Err(e) => {
                let msg = format!("Цикл налогов остановлен: {}", e.message);
                p.ctx.log(Level::Error, "dex.check", msg, None).await;
                return;
            }
        };
        let next = tokio::time::Instant::now() + interval;
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(next) => break,
                _ = wake.notified() => break,
                _ = p.signals.ready.notified() => {
                    // Пул стал достоверным: проверяем только ждущие монеты, у которых при прошлой проверке достоверного
                    // пула не было, а теперь есть (остальные ждущие — к следующему полному проходу).
                    let ready: Vec<(Address, String)> = waiting
                        .iter()
                        .filter(|(_, t, had)| !had && has_valid(&p, t))
                        .map(|(a, t, _)| (*a, t.clone()))
                        .collect();
                    if ready.is_empty() {
                        continue;
                    }
                    let again = check(&p, &ready, usdt, &mut config_seen).await;
                    waiting.retain(|(a, _, _)| !ready.iter().any(|(r, _)| r == a));
                    waiting.extend(again.waiting);
                }
            }
        }
    }
}

/// Есть ли у монеты достоверный пул. Ошибок нет.
fn has_valid(p: &Pools, ticker: &str) -> bool {
    p.reg.by_ticker(ticker).iter().any(|e| e.valid)
}

/// Проверяет монеты списка. Итог Known / Failed — в состояние и событие ядру; «ждёт пула» и «не задана настройка» —
/// прошлый итог не меняется, монета — в список ждущих. Ошибок нет.
async fn check(
    p: &Pools,
    list: &[(Address, String)],
    usdt: Address,
    seen: &mut HashSet<String>,
) -> Pass {
    let mut pass = Pass::default();
    for (i, (addr, ticker)) in list.iter().enumerate() {
        let block_time = match p.heads.fresh(&p.ctx) {
            Ok(h) => h.timestamp,
            Err(e) => {
                let msg = format!("Проверка налогов отложена: {}", e.message);
                p.signals.once(&p.ctx, Level::Warn, "dex.check", msg).await;
                pass.waiting
                    .extend(list[i..].iter().map(|(a, t)| (*a, t.clone(), false)));
                pass.no_pool += list.len() - i;
                return pass;
            }
        };
        let (c, out) = taxes::check_coin(
            &p.ctx, &p.reg, &p.checks, &p.tokens, ticker, *addr, usdt, block_time,
        )
        .await;
        match out {
            Outcome::Known(_) => pass.ok += 1,
            Outcome::Failed(reason) => {
                pass.failed += 1;
                let msg = format!("{ticker}: проверка налогов не прошла: {reason}");
                p.ctx.log(Level::Warn, "dex.check", msg, None).await;
            }
            Outcome::Wait(_) => {
                pass.no_pool += 1;
                pass.waiting
                    .push((*addr, ticker.clone(), has_valid(p, ticker)));
                continue;
            }
            Outcome::Config(why) => {
                pass.config += 1;
                pass.waiting
                    .push((*addr, ticker.clone(), has_valid(p, ticker)));
                if seen.insert(why.clone()) {
                    let text = format!("Проверка налогов: не задана настройка — {why}");
                    p.ctx
                        .log(Level::Error, "dex.check_not_configured", text.clone(), None)
                        .await;
                    let code = "dex.check_not_configured".to_string();
                    let _ = p.out.try_send(MarketEvent::Notice {
                        venue: p.ctx.venue(),
                        code,
                        text,
                    });
                }
                continue;
            }
        }
        p.checks.set_coin(c.clone());
        if p.out.try_send(MarketEvent::CoinCheck(c)).is_err() && p.out.is_closed() {
            return pass;
        }
    }
    pass
}
