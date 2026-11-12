//! Файл: venues/bsc_dex/check/quoter_one.rs
//! Что делает: сверка ОДНОГО пула с квотером и запись итога (вынесено из check/quoter_cycle.rs, решение 261 — файл
//! был больше 200 строк; логика та же): стороны независимо (решение 73), мелкая — без итога; решение 261 — голова H и
//! пул, прочитанный на ней, один раз на пул (check/quoter_block.rs), пауза и снятие стороны с алертами
//! (check/quoter_pause.rs), «не сверено (блок)» — итог прежний; сравнение памяти с сетью на H (memory) — причина в
//! журнал (dex.quoter_memory) и на экран «Глубина», без паузы.
//! Кто вызывает: check/quoter_cycle.rs.
//! От чего зависит: check/quoter_verify.rs, check/quoter_block.rs, check/quoter_pause.rs, check/quoter_side.rs,
//! check/quoter_tally.rs, quote/select.rs (deep_side), pools/depth_book.rs (missing_setting), check/state.rs.
//! Настройки: через вызываемые — dex.quoter_max_diff_pct, dex.quoter_memory_lag_blocks, money.max_per_trade_usd,
//! check.probe_usdt, глубина (dex.depth_band_pct и др.).

use crate::core::types::{PoolRef, Side};
use crate::storage::journal::Level;

use super::super::pools::listen::Pools;
use super::super::pools::progress::reason_key;
use super::super::pools::registry::Entry;
use super::super::quote::select;
use super::quoter_block::{self, Head};
use super::quoter_pause;
use super::quoter_sides::QuoterSides;
use super::quoter_tally::Tally;
use super::quoter_verify::{Verdict, usdt_decimals, verify};

/// Сверка одного пула и запись итога. Мелкий пул — итога нет (None), без журнала. До сводки первого прохода итог
/// идёт в неё (`first`), журнал по пулу — только после. Возвращает: была ли проверка (пула нет или он недостоверен —
/// false: пул не отмечается проверенным и сверится, как только станет достоверным, check/fresh.rs). Ошибок нет.
pub async fn one(p: &Pools, r: &PoolRef, first: &mut Option<Tally>) -> bool {
    // Данные пулов отстают (лента догоняет, решение 72) — вердикта нет: пул не отмечен проверенным, сверится после
    // снятия отметки (она будит циклы сигналом «пул готов»).
    if p.lag.busy() {
        return false;
    }
    let Some(e) = p.reg.get(r) else {
        return false;
    };
    if !e.valid {
        return false;
    }
    let dec = usdt_decimals(p).await;
    // Решение 261: голова H и пул, прочитанный на ней, — один раз на пул, лениво (первая глубокая сторона с заданными
    // настройками, check/quoter_verify.rs).
    let mut head = None;
    // Стороны — независимо (решение 73): сторона сверяется, когда она глубокая; мелкая — без итога, другая не меняется.
    let mut now = p.checks.pool(r).quoter;
    for side in [Side::Buy, Side::Sell] {
        let deep = match &dec {
            Ok(dec) => select::deep_side(
                &p.ctx,
                &p.reg,
                &p.tokens,
                &e,
                *dec,
                select::Need::Side(side),
            )
            .map(|shallow| (shallow, *dec)),
            Err(why) => Err(why.clone()),
        };
        let prev = now.side(side).clone();
        // Итог и имя незаданной настройки — из ошибки там, где она возникла (решение 75): по нему сторона
        // перепроверяется при задании именно этой настройки. Глубина — её настройки (pools/depth_book.rs).
        let v = match deep {
            Ok((None, dec)) => verify(p, &e, &mut head, dec, side).await,
            Ok((Some(_), _)) => {
                now.set(side, None);
                now.set_wait(side, None);
                now.set_note(side, None);
                continue;
            }
            Err(why) => Verdict::Fail((
                why,
                super::super::pools::depth_book::missing_setting(&p.ctx),
            )),
        };
        // Решение 261: пауза по расхождению на одном блоке, снятие — прошедшей сверкой; «не сверено (блок)» — итог прежний.
        let change = quoter_pause::apply(&mut now, side, &v);
        if let Verdict::NoBlock(why) = &v {
            let msg = format!(
                "{} {} ({}): {why}",
                e.family.title,
                e.ticker,
                quoter_pause::side_name(side)
            );
            p.signals.once(&p.ctx, Level::Info, "dex.quoter", msg).await;
        }
        match change {
            Some(c) => quoter_pause::alert(p, &e, side, &v, c).await,
            None => {
                if first.is_none()
                    && let Some(res) = now.side(side)
                    && super::quoter_side::changed(prev.as_ref(), res)
                {
                    super::quoter_side::log_change(p, &e, side, res).await;
                }
            }
        }
    }
    // Решение 261: память против сети на H — отдельная причина в журнал и на экран, без паузы и без алерта.
    if let Some(h) = &head {
        memory(p, &e, h, &mut now).await;
    }
    if let Some(t) = first {
        t.set(r, now.combined());
    }
    p.checks.update_pool(r, |f| f.quoter = now);
    true
}

/// Сравнение памяти с сетью на H (check/quoter_block.rs memory_note) → заметка пула и запись в журнал при смене
/// (появилась, сменился вид причины, ушла). Принимает: пулы, запись из памяти, голову, сверку сторон. Ошибок нет.
async fn memory(p: &Pools, e: &Entry, h: &Head, now: &mut QuoterSides) {
    // Пул на H не прочитан — сравнивать не с чем, прежняя заметка остаётся. Настройка не задана — сравнение не
    // выполняется, заметки нет (без записи в журнал).
    if !matches!(h, Head::Ready { .. }) {
        return;
    }
    if p.ctx.int(quoter_block::MEMORY_LAG).is_err() {
        now.memory = None;
        return;
    }
    let note = quoter_block::memory_note(&p.ctx, e, h);
    let changed = match (&now.memory, &note) {
        (None, None) => false,
        (Some(a), Some(b)) => reason_key(a) != reason_key(b),
        _ => true,
    };
    if changed {
        let (level, text) = match &note {
            Some(n) => (Level::Warn, format!("{} {}: {n}", e.family.title, e.ticker)),
            None => (
                Level::Info,
                format!("{} {}: память снова равна сети", e.family.title, e.ticker),
            ),
        };
        p.ctx.log(level, "dex.quoter_memory", text, None).await;
    }
    now.memory = note;
}
