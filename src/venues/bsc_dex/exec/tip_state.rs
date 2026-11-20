//! Файл: venues/bsc_dex/exec/tip_state.rs
//! Что делает: решение 437 (3) — состояние доли прибыли на чаевые (режим send.tip_mode = profit_share) по площадке
//! DEX: текущая доля s, последние чаевые, счётчики «победа / проигрыш по чаевым / проигрыш по скорости», побед
//! подряд, последний проигрыш. Хранится в SQLite (storage/venue_state.rs: площадка, вид «tip_share») — переживает
//! перезапуск; в памяти — копия по площадке (читается из базы при первом обращении).
//! Подстройка (/home/claude/src-ext/reports/arb-tips.md, раздел 3; утверждено Тиграном 20.11.2026):
//! - старт и сброс — минимум send.tip_share_min_pct (доля не задана → минимум);
//! - проигрыш по чаевым — s = min(s + send.tip_share_step_up_pct, send.tip_share_max_pct); дошла до максимума —
//!   событие send.tip_share_max (Telegram, notify/catalog.rs);
//! - send.tip_wins_to_step_down побед подряд — s = max(s − send.tip_share_step_down_pct, send.tip_share_min_pct);
//! - проигрыш по скорости — s не меняется; любой проигрыш обнуляет счёт побед подряд.
//!
//! Каждый проигрыш — строка журнала send.tip_loss (наш хеш, блок, перехватчик и его чаевые); в состоянии — последний.
//! Кто вызывает: exec/tip_cycle.rs (share, record), venue.rs (view, reset — экран «Каналы отправки»).
//! От чего зависит: storage/venue_state.rs, send/tip_mode.rs, send/stats.rs (событие), ctx.rs, core/types (TipShare).
//! Настройки: читает вызывающий (send/tip_mode.rs ShareCfg); view — send.tip_mode для показа режима.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{TipLoss, TipOutcome, TipShare};
use crate::gateway::errors::GatewayResult;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;
use crate::storage::venue_state;

use super::super::ctx::Ctx;
use super::super::send::stats::Stats;
use super::super::send::tip_mode::{self, ShareCfg};

/// Вид записи в venue_state и её ключ.
const KIND: &str = "tip_share";
const ID: &str = "state";

/// Площадка DEX → состояние (копия базы).
static ALL: LazyLock<Mutex<HashMap<&'static str, TipShare>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Состояние площадки: из памяти, иначе из базы (нет записи или не разобрана — пустое: доля = минимум). Ошибок нет.
async fn get(ctx: &Ctx) -> TipShare {
    if let Some(s) = mem(ctx) {
        return s;
    }
    let body = venue_state::load(ctx.journal.db(), ctx.place.id, KIND, ID).await;
    let st: TipShare = body
        .ok()
        .flatten()
        .and_then(|b| serde_json::from_str(&b).ok())
        .unwrap_or_default();
    let mut all = ALL.lock().unwrap_or_else(|p| p.into_inner());
    all.entry(ctx.place.id).or_insert(st).clone()
}

/// Копия из памяти. Ошибок нет.
fn mem(ctx: &Ctx) -> Option<TipShare> {
    let all = ALL.lock().unwrap_or_else(|p| p.into_inner());
    all.get(ctx.place.id).cloned()
}

/// Запоминает и пишет в базу. Сбой базы — в журнал (после перезапуска доля может быть старой). Ошибок нет.
async fn put(ctx: &Ctx, mut st: TipShare) -> TipShare {
    st.updated_at = Some(Utc::now());
    {
        let mut all = ALL.lock().unwrap_or_else(|p| p.into_inner());
        all.insert(ctx.place.id, st.clone());
    }
    let saved = match serde_json::to_string(&st) {
        Ok(body) => venue_state::save(ctx.journal.db(), ctx.place.id, KIND, ID, body)
            .await
            .map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    if let Err(e) = saved {
        let msg = format!("Доля чаевых не сохранена в SQLite: {e}");
        ctx.log(Level::Error, "send.tip_share", msg, None).await;
    }
    st
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
    let mut st = get(ctx).await;
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
    if let Some(l) = loss {
        let msg = loss_text(&l, was, now);
        ctx.log(Level::Warn, "send.tip_loss", msg, None).await;
        st.last_loss = Some(l);
    }
    put(ctx, st).await;
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

/// Состояние для экрана: сохранённое + режим (send.tip_mode) и причина, если режим не работает. Ошибок нет.
pub async fn view(ctx: &Ctx) -> TipShare {
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
    st
}

/// «Сбросить долю чаевых»: доля → минимум (не задана), побед подряд — 0; счётчики остаются. Принимает: окружение,
/// кто сбрасывает. Возвращает: состояние. Ошибок нет (сбой записи в базу — в журнале, put).
pub async fn reset(ctx: &Ctx, by: &str) -> GatewayResult<TipShare> {
    let mut st = get(ctx).await;
    let was = st.share_pct.map_or("минимум".to_string(), |s| format!("{s} %"));
    st.share_pct = None;
    st.streak = 0;
    put(ctx, st).await;
    let msg = format!("Доля чаевых сброшена к минимуму send.tip_share_min_pct ({by}); была {was}");
    ctx.log(Level::Info, "send.tip_share", msg, None).await;
    Ok(view(ctx).await)
}
