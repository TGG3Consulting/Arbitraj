//! Файл: venues/binance/discount_balance.rs
//! Что делает: решение 158 у Binance (как GT у Gate): хватает ли на споте монеты скидки комиссии (discountAsset из
//! ставок — BNB) на комиссию со скидкой. Фоновый круг читает её свободный остаток раз в venue.discount_balance_poll_s;
//! котировка (book.rs) берёт только память: остаток не меньше venue.discount_min_balance и замер не старше двух
//! кругов — расчёт со скидкой; иначе — полная ставка в монете сделки. Так, когда BNB кончится, сделки идут дальше по
//! полной ставке, а не останавливаются.
//! Кто вызывает: subs.rs (запуск с первой подпиской), book.rs (ok — в котировке).
//! От чего зависит: account.rs (баланс спота), fees.rs (монета скидки), ctx.rs, settings/wait.rs.
//! Настройки: venue.discount_min_balance [binance], venue.discount_balance_poll_s [binance]. Не заданы — монеты скидки
//! «не хватает»: полная ставка.
//!
//! Официально: GET /api/v3/account (вес 20) — balances[].free; faqs/commission_faq — при нехватке BNB «the full
//! commission will be taken out of your received amount». Имя монеты — discount.discountAsset ответа
//! account/commission (fees.rs), не настройка.

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use rust_decimal::Decimal;

use crate::settings::wait::{Note, until_ok};
use crate::storage::journal::Level;

use super::account;
use super::ctx::Ctx;

/// Последний замер свободного остатка монеты скидки. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct DiscountBalance {
    last: Arc<RwLock<Option<(Decimal, Instant)>>>,
}

impl DiscountBalance {
    /// Хватает ли монеты скидки сейчас. Принимает: окружение. Возвращает: (да/нет, пояснение). Ошибок нет.
    pub fn ok(&self, ctx: &Ctx) -> (bool, String) {
        let (Ok(min), Ok(poll)) = (
            ctx.dec("venue.discount_min_balance"),
            ctx.int("venue.discount_balance_poll_s"),
        ) else {
            return (
                false,
                "не заданы venue.discount_min_balance / venue.discount_balance_poll_s".into(),
            );
        };
        let last = *self.last.read().unwrap_or_else(|p| p.into_inner());
        match last {
            None => (false, "остаток монеты скидки ещё не прочитан".into()),
            Some((_, at)) if at.elapsed() > Duration::from_secs(poll.saturating_mul(2).max(1)) => {
                (false, "замер остатка монеты скидки устарел".into())
            }
            Some((v, _)) if v < min => (
                false,
                format!("остаток {v} меньше venue.discount_min_balance {min}"),
            ),
            Some((v, _)) => (true, format!("остаток {v}")),
        }
    }

    /// Фоновый круг: читает остаток монеты скидки раз в venue.discount_balance_poll_s. Монета ещё не известна (ставок
    /// нет) или чтение не удалось — замер не обновляется. Работает до остановки задачи.
    pub async fn run(self, ctx: Ctx, fees: super::fees::Fees) {
        loop {
            let note = Note {
                journal: &ctx.journal,
                kind: "venue.discount_balance",
                venue: Some(super::ctx::ID),
            };
            let read = || ctx.int("venue.discount_balance_poll_s");
            let Some(poll) =
                until_ok(&ctx.settings, note, "Остаток монеты скидки Binance", read).await
            else {
                return;
            };
            if let Some(asset) = fees.discount_asset() {
                match account::balances(&ctx).await {
                    Ok(list) => {
                        let v = list
                            .iter()
                            .filter(|b| b.asset == asset)
                            .map(|b| b.free)
                            .sum::<Decimal>();
                        *self.last.write().unwrap_or_else(|p| p.into_inner()) =
                            Some((v, Instant::now()));
                    }
                    Err(e) => {
                        let m = format!("Остаток {asset} не прочитан: {}", e.message);
                        ctx.log(Level::Warn, "venue.discount_balance", m).await;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(poll.max(1))).await;
        }
    }
}
