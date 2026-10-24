//! Файл: venues/bsc_dex/check/tax_pass.rs
//! Что делает: проверка налогов списка монет (один проход или его часть). Итог Known / Failed — в состояние и событие
//! ядру; «ждёт» (нет достоверного пула, сбой узла — 429, нет связи) и «не задана настройка» — прошлый итог не
//! меняется, монета — в список ждущих (не вердикт, не чёрный список). Журнал по монете — только при смене состояния
//! (прошла ↔ не прошла, сменилась причина без учёта цифр) и не в первом проходе (там — одна сводка, tax_cycle.rs).
//! Ошибка настройки — одна запись в журнал и одно событие dex.check_not_configured на каждую разную причину.
//! Кто вызывает: check/tax_cycle.rs.
//! От чего зависит: check/taxes.rs, check/state.rs, pools/listen.rs (Pools), pools/progress.rs, node/heads.rs.
//! Настройки: через вызываемые.

use std::collections::HashSet;

use alloy_primitives::Address;

use crate::core::types::CheckState;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::super::pools::listen::Pools;
use super::super::pools::progress::reason_key;
use super::taxes::{self, Outcome};

/// Почему монета ждёт.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// Нет достоверного пула — проверка по сигналу «пул готов».
    NoPool,
    /// Пул был, помешал узел или окружение (429, нет связи) — повтор через tax.retry_s.
    Node,
    /// Не задана настройка — до следующего полного прохода.
    Config,
}

/// Итоги прохода.
#[derive(Default)]
pub struct Pass {
    /// Ждущие монеты: адрес, тикер, почему.
    pub waiting: Vec<(Address, String, Why)>,
    pub ok: usize,
    pub failed: usize,
    pub no_pool: usize,
    pub config: usize,
}

/// Есть ли у монеты достоверный пул. Ошибок нет.
pub fn has_valid(p: &Pools, ticker: &str) -> bool {
    p.reg.by_ticker(ticker).iter().any(|e| e.valid)
}

/// Проверяет монеты списка (см. шапку). Принимает: окружение, монеты, USDT, уже записанные ошибки настроек, первый ли
/// это проход. Ошибок нет.
pub async fn check(
    p: &Pools,
    list: &[(Address, String)],
    usdt: Address,
    seen: &mut HashSet<String>,
    first: bool,
) -> Pass {
    let mut pass = Pass::default();
    for (i, (addr, ticker)) in list.iter().enumerate() {
        let block_time = match p.heads.fresh(&p.ctx) {
            Ok(h) => h.timestamp,
            Err(e) => {
                let msg = format!("Проверка налогов отложена: {}", e.message);
                p.signals.once(&p.ctx, Level::Warn, "dex.check", msg).await;
                for (a, t) in &list[i..] {
                    pass.waiting.push((*a, t.clone(), Why::Node));
                }
                pass.no_pool += list.len() - i;
                return pass;
            }
        };
        let (c, out) = taxes::check_coin(
            &p.ctx, &p.reg, &p.checks, &p.tokens, ticker, *addr, usdt, block_time,
        )
        .await;
        let prev = p.checks.coin(ticker).map(|c| c.taxes);
        match out {
            Outcome::Known(_) => {
                pass.ok += 1;
                if !first && matches!(prev, Some(CheckState::Failed { .. })) {
                    let msg = format!("{ticker}: проверка налогов снова прошла");
                    p.ctx.log(Level::Info, "dex.check", msg, None).await;
                }
            }
            Outcome::Failed(reason) => {
                pass.failed += 1;
                let same = matches!(&prev, Some(CheckState::Failed { reason: r }) if reason_key(r) == reason_key(&reason));
                if !first && !same {
                    let msg = format!("{ticker}: проверка налогов не прошла: {reason}");
                    p.ctx.log(Level::Warn, "dex.check", msg, None).await;
                }
            }
            Outcome::Wait(_) => {
                pass.no_pool += 1;
                let why = if has_valid(p, ticker) {
                    Why::Node
                } else {
                    Why::NoPool
                };
                pass.waiting.push((*addr, ticker.clone(), why));
                continue;
            }
            Outcome::Config(why) => {
                pass.config += 1;
                pass.waiting.push((*addr, ticker.clone(), Why::Config));
                if seen.insert(why.clone()) {
                    let text = format!("Проверка налогов: не задана настройка — {why}");
                    p.ctx
                        .log(Level::Error, "dex.check_not_configured", text.clone(), None)
                        .await;
                    let code = "dex.check_not_configured".to_string();
                    let venue = p.ctx.venue();
                    let _ = p.out.try_send(MarketEvent::Notice { venue, code, text });
                }
                continue;
            }
        }
        p.checks.set_coin(c.clone());
        // С ожиданием места в канале: итог проверки не должен теряться, когда ядро занято (первый проход — сотни монет).
        if p.out.send(MarketEvent::CoinCheck(c)).await.is_err() {
            return pass;
        }
    }
    pass
}
