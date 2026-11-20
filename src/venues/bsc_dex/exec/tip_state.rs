//! Файл: venues/bsc_dex/exec/tip_state.rs
//! Что делает: решение 437 (3) — состояние доли прибыли на чаевые (режим send.tip_mode = profit_share) по площадке
//! DEX: текущая доля s, последние чаевые, счётчики «победа / проигрыш по чаевым / проигрыш по скорости», побед
//! подряд, последний проигрыш. Хранение (память + SQLite, изменение под замком) — exec/tip_state_store.rs.
//! Ревью 437: п. 6 — состояние и сброс только у площадки dex.gas_mode = eip1559_l1 (иначе NotApplicable, 400 в API);
//! п. 8 — победа тоже строка журнала send.tip_share (без Telegram).
//! Подстройка (/home/claude/src-ext/reports/arb-tips.md, раздел 3; утверждено Тиграном 20.11.2026):
//! - старт и сброс — минимум send.tip_share_min_pct (доля не задана → минимум);
//! - проигрыш по чаевым — s = min(s + send.tip_share_step_up_pct, send.tip_share_max_pct); дошла до максимума —
//!   событие send.tip_share_max (Telegram, notify/catalog.rs);
//! - send.tip_wins_to_step_down побед подряд — s = max(s − send.tip_share_step_down_pct, send.tip_share_min_pct);
//! - проигрыш по скорости — s не меняется; любой проигрыш обнуляет счёт побед подряд.
//!
//! Каждый проигрыш — строка журнала send.tip_loss (наш хеш, блок, перехватчик и его чаевые); в состоянии — последний.
//! Кто вызывает: exec/tip_cycle.rs (share, record), venue.rs (view, reset — экран «Каналы отправки»).
//! От чего зависит: exec/tip_state_store.rs, send/{tip_mode, gas_mode, stats}.rs, ctx.rs, core/types (TipShare).
//! Настройки: читает вызывающий (send/tip_mode.rs ShareCfg); view — send.tip_mode для показа режима; dex.gas_mode.

use rust_decimal::Decimal;

use crate::core::types::{TipLoss, TipOutcome, TipShare};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::send::gas_mode::{self, GasMode};
use super::super::send::stats::Stats;
use super::super::send::tip_mode::{self, ShareCfg};
use super::tip_state_store::{change, get};

/// Ревью 437, п. 6: доля чаевых — только у площадки dex.gas_mode = eip1559_l1 (Arbitrum). Принимает: окружение.
/// Ошибки: NotApplicable (400 в API) — модель газа другая или не читается (с причиной).
pub fn applies(ctx: &Ctx) -> GatewayResult<()> {
    let why = match gas_mode::read(ctx) {
        Ok(GasMode::Eip1559L1) => return Ok(()),
        Ok(_) => "доля чаевых (PGA) — только у площадки с dex.gas_mode = eip1559_l1".to_string(),
        Err(e) => format!("доля чаевых (PGA): модель газа не прочитана — {}", e.message),
    };
    Err(ctx.err(ErrorCode::NotApplicable, why))
}

/// Текущая доля, % — в пределах [минимум, максимум] настроек (доля не задана — минимум). Принимает: окружение,
/// настройки profit_share. Ошибок нет.
pub async fn share(ctx: &Ctx, c: &ShareCfg) -> Decimal {
    clamp(get(ctx).await.share_pct, c)
}

/// Доля в пределах настроек: не задана — минимум. Ошибок нет.
fn clamp(s: Option<Decimal>, c: &ShareCfg) -> Decimal {
    s.unwrap_or(c.min_pct).max(c.min_pct).min(c.max_pct)
}

/// Итог сделки цикла: счётчики, подстройка доли, последние чаевые, журнал проигрыша, событие «доля дошла до
/// максимума». Принимает: окружение, счётчики каналов (событие), настройки, наши чаевые (wei за газ), итог, проигрыш
/// (что нашли по блоку; None — победа). Ошибок нет.
pub async fn record(
    ctx: &Ctx,
    stats: &Stats,
    c: &ShareCfg,
    (tip, out): (u128, TipOutcome),
    loss: Option<TipLoss>,
) {
    // Ревью 439 (6): состояние не загружено из базы — итог не учтён (причина — в журнале change).
    let Ok((was, now)) = change(ctx, |st| {
        let was = clamp(st.share_pct, c);
        let mut now = was;
        st.last_tip_gwei = tip_mode::to_gwei(tip);
        match out {
            TipOutcome::Win => {
                st.wins += 1;
                st.streak += 1;
                if st.streak >= c.wins_to_down {
                    now = (was - c.down_pct).max(c.min_pct);
                    st.streak = 0;
                }
            }
            TipOutcome::LostTip => {
                st.lost_tip += 1;
                st.streak = 0;
                now = (was + c.up_pct).min(c.max_pct);
            }
            TipOutcome::LostSpeed => {
                st.lost_speed += 1;
                st.streak = 0;
            }
        }
        st.share_pct = Some(now);
        if let Some(l) = &loss {
            st.last_loss = Some(l.clone());
        }
        (was, now)
    })
    .await
    else {
        return;
    };
    // Журнал — после изменения и записи (ревью 437, п. 3); победа — тоже строка send.tip_share (п. 8, без Telegram):
    // по ней экран «Доля чаевых» перечитывается.
    match &loss {
        Some(l) => ctx.log(Level::Warn, "send.tip_loss", loss_text(l, was, now), None).await,
        None => {
            let gwei = tip_mode::to_gwei(tip).map_or("—".to_string(), |g| g.to_string());
            let msg = format!(
                "Победа сделки DEX ↔ DEX: чаевые {gwei} gwei; доля чаевых {was} % → {now} %"
            );
            ctx.log(Level::Info, "send.tip_share", msg, None).await;
        }
    }
    if now >= c.max_pct && was < c.max_pct {
        stats.emit(MarketEvent::Notice {
            venue: ctx.venue(),
            code: "send.tip_share_max".into(),
            text: format!(
                "Доля прибыли на чаевые сделок DEX ↔ DEX дошла до максимума \
                 send.tip_share_max_pct ({now} %) — проигрыши по чаевым подряд; выше не поднимается"
            ),
        });
    }
}

/// Строка журнала проигрыша. Ошибок нет.
fn loss_text(l: &TipLoss, was: Decimal, now: Decimal) -> String {
    let kind = match l.outcome {
        TipOutcome::LostTip => "по чаевым",
        _ => "по скорости",
    };
    let rival = match (&l.rival, l.rival_tip_gwei) {
        (Some(h), Some(t)) => format!("перехватчик {h}, его чаевые {t} gwei"),
        (Some(h), None) => format!("перехватчик {h}, чаевые не прочитаны"),
        _ => "перехватчика по пулам маршрута в блоке нет".to_string(),
    };
    format!(
        "Проигрыш {kind}: tx {} (блок {}), наши чаевые {} gwei; {rival}; \
         доля чаевых {was} % → {now} %",
        l.ours, l.block, l.our_tip_gwei
    )
}

/// Состояние для экрана: сохранённое + режим (send.tip_mode) и причина, если режим не работает.
/// Ошибки: NotApplicable — площадка не eip1559_l1 (applies, ревью 437, п. 6).
pub async fn view(ctx: &Ctx) -> GatewayResult<TipShare> {
    applies(ctx)?;
    let mut st = get(ctx).await;
    st.venue = ctx.place.id.to_string();
    match tip_mode::read(ctx) {
        Ok(m) => {
            st.mode = Some(m.as_str().to_string());
            st.note = None;
        }
        Err(e) => {
            st.mode = ctx.text("send.tip_mode").ok();
            st.note = Some(e.message);
        }
    }
    Ok(st)
}

/// «Сбросить долю чаевых»: доля → минимум (не задана), побед подряд — 0; счётчики остаются. Принимает: окружение,
/// кто сбрасывает. Возвращает: состояние. Ошибки: NotApplicable — площадка не eip1559_l1 (сбой записи в базу — в
/// журнале, persist); Internal — состояние не загружено из SQLite (ревью 439 (6): сброс не выполнен).
pub async fn reset(ctx: &Ctx, by: &str) -> GatewayResult<TipShare> {
    applies(ctx)?;
    let was = change(ctx, |st| {
        let was = st.share_pct.map_or("минимум".to_string(), |s| format!("{s} %"));
        st.share_pct = None;
        st.streak = 0;
        was
    })
    .await
    .map_err(|why| ctx.err(ErrorCode::Internal, format!("сброс не выполнен: {why}")))?;
    let msg = format!("Доля чаевых сброшена к минимуму send.tip_share_min_pct ({by}); была {was}");
    ctx.log(Level::Info, "send.tip_share", msg, None).await;
    view(ctx).await
}
