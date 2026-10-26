//! Файл: venues/bsc_dex/pools/depth_log.rs
//! Что делает: журнал смен «глубокий ↔ мелкий» глубины пула (решение 73): запись — только при реальной смене вердикта
//! по сравнению с последним ЗАПИСАННЫМ, по пулу не чаще dex.depth_log_min_s. Смена внутри окна не выбрасывается, а
//! откладывается до конца окна: тогда пишется последнее состояние (если оно снова как записанное — ничего).
//! Первый расчёт пула — точка отсчёта, без записи. Настройка необязательна: не задана — записей нет.
//! Кто вызывает: pools/depth_book.rs (after — после каждого пересчёта).
//! От чего зависит: depth_book.rs (Book, Slot), ctx.rs.
//! Настройки: dex.depth_log_min_s [bsc_dex].

use std::time::{Duration, Instant};

use crate::core::types::PoolRef;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::depth_book::{Book, Slot};

/// Что о пуле уже записано.
#[derive(Debug, Default)]
pub struct Said {
    /// Когда писали последний раз.
    at: Option<Instant>,
    /// Записанный вердикт (покупка, продажа); None — точки отсчёта ещё нет.
    flags: Option<(bool, bool)>,
    /// Ждёт конца окна отложенная запись.
    pending: bool,
}

/// После пересчёта глубины пула `r` (см. шапку). Вне среды tokio — без записи. Ошибок нет.
pub fn after(book: &Book, ctx: &Ctx, r: &PoolRef) {
    let Ok(min) = ctx.int("dex.depth_log_min_s") else {
        return;
    };
    let mut m = book.lock();
    let Some(s) = m.get_mut(r) else { return };
    let now = (s.depth.deep_buy, s.depth.deep_sell);
    match s.said.flags {
        None => {
            s.said.flags = Some(now);
            return;
        }
        Some(f) if f == now => return,
        Some(_) => {}
    }
    if s.said.pending {
        return;
    }
    let wait = s
        .said
        .at
        .map(|t| Duration::from_secs(min).saturating_sub(t.elapsed()))
        .unwrap_or_default();
    if wait.is_zero() {
        write(ctx, s);
        return;
    }
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    s.said.pending = true;
    let (book, ctx, r) = (book.clone(), ctx.clone(), r.clone());
    rt.spawn(async move {
        tokio::time::sleep(wait).await;
        let mut m = book.lock();
        let Some(s) = m.get_mut(&r) else { return };
        s.said.pending = false;
        if s.said.flags != Some((s.depth.deep_buy, s.depth.deep_sell)) {
            write(&ctx, s);
        }
    });
}

/// Запись о текущем вердикте пула (отдельной задачей: вызывающий держит блокировку). Ошибок нет.
fn write(ctx: &Ctx, s: &mut Slot) {
    let d = &s.depth;
    s.said.at = Some(Instant::now());
    s.said.flags = Some((d.deep_buy, d.deep_sell));
    let word = |deep: bool| {
        if deep {
            "глубокий"
        } else {
            "мелкий"
        }
    };
    let msg = format!(
        "{}: покупка — {} ({} USDT), продажа — {} ({} USDT) в полосе ±{}% (порог dex.min_pool_liquidity_usd {}, \
         вниз — ниже порога на dex.depth_hysteresis_pct {}%){}",
        s.label,
        word(d.deep_buy),
        d.band.buy.round_dp(2),
        word(d.deep_sell),
        d.band.sell.round_dp(2),
        d.band_pct,
        d.min,
        d.hyst,
        if d.band.full {
            ""
        } else {
            "; полоса прочитана не целиком — глубина до края прочитанной карты"
        }
    );
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        let ctx = ctx.clone();
        rt.spawn(async move { ctx.log(Level::Info, "dex.pool", msg, None).await });
    }
}
