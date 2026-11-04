//! Файл: venues/gate/gt_balance.rs
//! Что делает: решение 158 (Тигран 05.11): баланс GT на споте Gate — хватает ли его на комиссию со скидкой GT.
//! Фоновый круг читает свободный GT (GET /spot/accounts?currency=GT) раз в venue.gt_balance_poll_s; котировка (book.rs)
//! берёт только память: GT не меньше venue.gt_min_balance и замер не старше двух кругов — комиссия считается со
//! скидкой GT (gt_taker_fee, в GT); иначе — по обычной ставке в монете сделки (худший случай). Так, когда GT кончится,
//! сделки Gate сами продолжают идти по обычной ставке (комиссия в монете / USDT), а не останавливаются.
//! Кто вызывает: subs.rs (запуск круга с первой подпиской), book.rs (ok — в котировке).
//! От чего зависит: account.rs (баланс спота), ctx.rs, settings/wait.rs.
//! Настройки: venue.gt_min_balance [gate] (GT), venue.gt_balance_poll_s [gate] (с). Не заданы — GT считается
//! недостаточным: комиссия по обычной ставке (запись в журнал один раз).
//!
//! Официальная документация https://www.gate.com/docs/developers/apiv4/en/ : «List spot trading accounts»
//! GET /spot/accounts?currency= (available, locked); лимит Spot «all other endpoints» 200 r/10s — круг раз в несколько
//! секунд берёт доли процента. Что Gate делает при нехватке GT для скидки — в документации не описано: поэтому при
//! нехватке расчёт идёт по обычной ставке (комиссия выше или равна факту), факт комиссии — из ордера (order_state.rs).

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use rust_decimal::Decimal;

use crate::settings::wait::{Note, until_ok};
use crate::storage::journal::Level;

use super::account;
use super::ctx::Ctx;

/// Тикер GT у Gate (монета скидки; имя валюты в API — «GT»). Не настройка: это имя валюты API Gate.
const GT: &str = "GT";

/// Последний замер свободного GT. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct GtBalance {
    last: Arc<RwLock<Option<(Decimal, Instant)>>>,
}

impl GtBalance {
    /// Хватает ли GT на скидку сейчас: замер есть, не старше двух кругов, GT ≥ venue.gt_min_balance.
    /// Принимает: окружение. Возвращает: (да/нет, пояснение для журнала/плана). Ошибок нет.
    pub fn ok(&self, ctx: &Ctx) -> (bool, String) {
        let (Ok(min), Ok(poll)) = (
            ctx.dec("venue.gt_min_balance"),
            ctx.int("venue.gt_balance_poll_s"),
        ) else {
            return (
                false,
                "не заданы venue.gt_min_balance / venue.gt_balance_poll_s".into(),
            );
        };
        let last = *self.last.read().unwrap_or_else(|p| p.into_inner());
        match last {
            None => (false, "баланс GT ещё не прочитан".into()),
            Some((_, at)) if at.elapsed() > Duration::from_secs(poll.saturating_mul(2).max(1)) => {
                (false, "замер баланса GT устарел".into())
            }
            Some((gt, _)) if gt < min => {
                (false, format!("GT {gt} меньше venue.gt_min_balance {min}"))
            }
            Some((gt, _)) => (true, format!("GT {gt}")),
        }
    }

    /// Фоновый круг: читает свободный GT раз в venue.gt_balance_poll_s. Ошибка чтения — замер не обновляется (через два
    /// круга считается устаревшим — комиссия по обычной ставке). Возвращает: только при остановке системы.
    pub async fn run(self, ctx: Ctx) {
        loop {
            let note = Note {
                journal: &ctx.journal,
                kind: "venue.gt_balance",
                venue: Some(super::ctx::ID),
            };
            let read = || ctx.int("venue.gt_balance_poll_s");
            let Some(poll) = until_ok(
                &ctx.settings,
                note,
                "Баланс GT Gate (скидка комиссии)",
                read,
            )
            .await
            else {
                return;
            };
            match account::balances(&ctx, Some(GT)).await {
                Ok(list) => {
                    let gt = list
                        .iter()
                        .filter(|b| b.asset == GT)
                        .map(|b| b.free)
                        .sum::<Decimal>();
                    *self.last.write().unwrap_or_else(|p| p.into_inner()) =
                        Some((gt, Instant::now()));
                }
                Err(e) => {
                    let m = format!("Баланс GT не прочитан: {}", e.message);
                    let _ = ctx
                        .journal
                        .write(
                            Level::Warn,
                            "venue.gt_balance",
                            Some(super::ctx::ID),
                            m,
                            None,
                        )
                        .await;
                }
            }
            tokio::time::sleep(Duration::from_secs(poll.max(1))).await;
        }
    }
}
