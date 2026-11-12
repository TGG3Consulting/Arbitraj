//! Файл: venues/bsc_dex/check/quoter_pause.rs
//! Что делает: решение 261, часть 2 — пауза стороны пула по сверке с квотером на одном блоке:
//! - итог стороны по вердикту (apply, чистая): прошла — итог Ok, пауза снята; расхождение на одном блоке — итог с
//!   причиной (сторона исключена из котировок и сделок, quote/select.rs), пауза поставлена; иная причина — итог с
//!   причиной, отметка паузы не трогается (снимает её только прошедшая сверка); «не сверено (блок)» — итог и пауза
//!   прежние, причина — в заметке стороны (экран «Монеты»);
//! - алерт в Telegram ровно на смену паузы (ушла на паузу / пауза снята) — по одному сообщению на событие, без повторов
//!   каждые dex.quoter_check_s: события pool.quoter_paused / pool.quoter_resumed (notify/catalog.rs; включаются и
//!   выключаются на экране «Уведомления»), и запись в журнал (dex.quoter_pause).
//!
//! Кто вызывает: check/quoter_cycle.rs (one).
//! От чего зависит: check/quoter_sides.rs, check/quoter_verify.rs (Verdict),
//! pools/listen.rs (Pools: канал событий ядру), pools/depth_book.rs (label), tokens.rs, num.rs.
//! Настройки: нет (вкл/выкл событий — notify.event.enabled с областью события).

use alloy_primitives::{Address, U256};

use crate::core::types::Side;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::super::num;
use super::super::pools::depth_book::label;
use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;
use super::quoter_sides::QuoterSides;
use super::quoter_verify::Verdict;

/// Событие уведомлений: сторона пула ушла на паузу.
pub const CODE_PAUSED: &str = "pool.quoter_paused";
/// Событие уведомлений: пауза стороны пула снята.
pub const CODE_RESUMED: &str = "pool.quoter_resumed";

/// Смена паузы стороны.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Paused,
    Resumed,
}

/// Итог стороны по вердикту (см. шапку). Принимает: сверку сторон, сторону, вердикт. Возвращает: смену паузы (None —
/// не менялась). Ошибок нет.
pub fn apply(q: &mut QuoterSides, side: Side, v: &Verdict) -> Option<Change> {
    let was = q.paused(side);
    match v {
        Verdict::Ok { .. } => {
            q.set(side, Some(Ok(())));
            q.set_wait(side, None);
            q.set_note(side, None);
            q.set_paused(side, false);
            was.then_some(Change::Resumed)
        }
        Verdict::Pause { why, .. } => {
            q.set(side, Some(Err(why.clone())));
            q.set_wait(side, None);
            q.set_note(side, None);
            q.set_paused(side, true);
            (!was).then_some(Change::Paused)
        }
        Verdict::Fail((why, name)) => {
            q.set(side, Some(Err(why.clone())));
            q.set_wait(side, name.clone());
            q.set_note(side, None);
            None
        }
        Verdict::NoBlock(why) => {
            q.set_note(side, Some(why.clone()));
            None
        }
    }
}

/// Сторона по-русски. Ошибок нет.
pub fn side_name(s: Side) -> &'static str {
    if s == Side::Buy {
        "покупка"
    } else {
        "продажа"
    }
}

/// Текст алерта (чистая, тестируется). Принимает: запись, сторону, вердикт, смену, знаки токена выхода (None — не
/// известны: только минимальные единицы). Возвращает: текст. Ошибок нет.
pub fn text(e: &Entry, side: Side, v: &Verdict, c: Change, out_dec: Option<u8>) -> String {
    let pool = format!(
        "Пул: {}\nDEX: {}\nМонета: {}\nСторона: {}",
        label(&e.pool.reference),
        e.family.title,
        e.ticker,
        side_name(side)
    );
    match (c, v) {
        (
            Change::Paused,
            Verdict::Pause {
                why,
                block,
                usd,
                nums,
            },
        ) => {
            let mut t = format!(
                "🚨🚨🚨 ПАУЗА ПУЛА!!! Расчёт пула разошёлся с квотером НА ОДНОМ БЛОКЕ!!!\n{pool}\nСумма сверки: {} USDT",
                usd.round_dp(2)
            );
            if let Some(n) = nums {
                t.push_str(&format!(
                    "\nНаш расчёт: {}\nКвотер: {}\nРасхождение: {}% (допуск dex.quoter_max_diff_pct {}%)",
                    amount(n.local, out_dec),
                    amount(n.remote, out_dec),
                    n.pct.round_dp(4),
                    n.limit
                ));
            }
            t.push_str(&format!(
                "\nБлок: {block}\nПричина: {why}\nЧто делает бот: сторона пула исключена из котировок и сделок; пауза \
                 снимется сама, когда следующая сверка на одном блоке пройдёт."
            ));
            t
        }
        (_, Verdict::Ok { block }) => format!(
            "Пауза снята: сверка с квотером на одном блоке прошла.\n{pool}\nБлок: {block}\nСторона снова в котировках и \
             сделках."
        ),
        (_, v) => format!("{pool}\n{v:?}"),
    }
}

/// Сумма в минимальных единицах и, если знаки известны, в токенах. Ошибок нет.
fn amount(v: U256, dec: Option<u8>) -> String {
    match dec.and_then(|d| num::to_decimal(v, d)) {
        Some(x) => format!("{x} ({v} мин. ед.)"),
        None => format!("{v} мин. ед."),
    }
}

/// Токен выхода стороны: покупка — монета, продажа — встречный токен пула. Ошибок нет.
fn out_token(e: &Entry, side: Side) -> Address {
    if side == Side::Buy { e.coin } else { e.usdt }
}

/// Алерт и запись в журнал о смене паузы (одно сообщение на событие). Ошибок нет: канал ядра закрыт — молча.
pub async fn alert(p: &Pools, e: &Entry, side: Side, v: &Verdict, c: Change) {
    let dec = p.tokens.decimals_known(out_token(e, side));
    let msg = text(e, side, v, c, dec);
    let (level, code) = match c {
        Change::Paused => (Level::Error, CODE_PAUSED),
        Change::Resumed => (Level::Info, CODE_RESUMED),
    };
    p.ctx
        .log(level, "dex.quoter_pause", msg.clone(), None)
        .await;
    let ev = MarketEvent::Notice {
        venue: p.ctx.venue(),
        code: code.to_string(),
        text: msg,
    };
    let _ = p.out.send(ev).await;
}
