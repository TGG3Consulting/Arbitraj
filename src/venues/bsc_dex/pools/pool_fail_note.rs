//! Файл: venues/bsc_dex/pools/pool_fail_note.rs
//! Что делает: решение 339 — запись неудачи чтения пула (отметка pool_fail.rs Fails в реестре), журнал, уведомление
//! Telegram и очередь повтора. Вынесено из pool_fail.rs (ревью t153: файл вырос). Пути pool_fail::note / block_failed /
//! paused / requeue / Reason — реэкспорт в pool_fail.rs.
//! • note: журнал — на первую неудачу и на смену КОДА причины (Н4: не текста — текст ошибки узла меняется от раза к
//!   разу); ошибка связи или лимита узла (pool_fail::is_link) пул не отмечает журналом — одна запись на сбой узла.
//! • Н7: пул не читается дольше dex.pool_unread_alert_s (от первой неудачи эпизода) — одно уведомление dex.pool_unread
//!   (журнал dex.pool + MarketEvent::Notice ядру → Telegram по коду события), до удачного чтения повтора нет. Проверка
//!   идёт на каждой неудаче пула, поэтому пул на паузе уведомляется на первой неудаче после порога.
//! • requeue (Б1): пул — в очередь повтора; при заданной dex.pool_fail_pause_ms пул, брошенный из-за НЕЗАДАННОЙ
//!   настройки, остаётся брошенным (вернётся, когда настройку зададут, — revive.rs; иначе каждое событие гоняло бы его
//!   по кругу «повтор → та же ошибка настройки → брошен»). Не задана — ровно stale_read::requeue, как раньше.
//! • Адрес пула в текстах журнала и уведомления — depth_book::label (как у dex.pool_refresh): под одной подписью
//!   «вид тикер» бывает несколько пулов.
//! Кто вызывает: через pool_fail.rs — listen_log.rs, listen_site.rs, reload.rs (retry), per_block.rs, per_block_par.rs,
//! per_block_lb.rs, per_block_balancer.rs, session.rs, session_lanes.rs.
//! От чего зависит: pool_fail.rs (Fails, base, cap, alert_after, is_link), listen.rs (Pools), registry.rs, discover.rs
//! (Found), stale_read.rs (requeue), reload.rs (Missing), revive.rs (Abandoned), gateway/events.rs (Notice),
//! storage/journal.
//! Настройки: через pool_fail.rs — dex.pool_fail_pause_ms, dex.pool_fail_pause_max_ms, dex.pool_unread_alert_s.

use std::time::Instant;

use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::depth_book::label;
use super::discover::Found;
use super::listen::Pools;
use super::pool_fail::{Fail, alert_after, base, cap, is_link};
use super::reload::Missing;
use super::revive::Abandoned;

/// Код события уведомлений «пул не читается» (notify/catalog.rs).
pub const CODE_UNREAD: &str = "dex.pool_unread";

/// Причина неудачи: код ошибки шлюза (None — причина без кода: ответ пачки не по форме, откат вызова) и текст.
pub trait Reason {
    /// Код ошибки шлюза; None — текст без кода.
    fn code(&self) -> Option<ErrorCode>;
    /// Текст причины.
    fn text(&self) -> &str;
}

impl Reason for GatewayError {
    fn code(&self) -> Option<ErrorCode> {
        Some(self.code)
    }
    fn text(&self) -> &str {
        &self.message
    }
}

impl Reason for String {
    fn code(&self) -> Option<ErrorCode> {
        None
    }
    fn text(&self) -> &str {
        self
    }
}

/// Что делает: пул на паузе после неудач (изоляция включена и пауза не прошла) — читать его сейчас нельзя.
/// Принимает: окружение, ссылку. Возвращает: true — на паузе. Ошибок нет.
pub fn paused(p: &Pools, r: &PoolRef) -> bool {
    // Н5: без отметок — без чтения настройки и замка.
    p.reg.fails().any() && base(&p.ctx).is_some() && !p.reg.fails().due(r, Instant::now())
}

/// Что делает: неудача чтения пула при включённой изоляции: отметка (пауза растёт, у ошибки узла — нет), журнал — на
/// первую неудачу и на смену кода причины (у ошибки узла — одна запись на сбой узла), уведомление dex.pool_unread
/// после порога. Принимает: окружение, ссылку, имя вида, тикер, причину. Возвращает: true — изоляция включена
/// (вызывающий НЕ рвёт сессию); false — не задана dex.pool_fail_pause_ms (как раньше, ничего не записано). Ошибок нет.
pub async fn note<R: Reason + ?Sized>(
    p: &Pools,
    r: &PoolRef,
    title: &str,
    ticker: &str,
    why: &R,
) -> bool {
    let Some(b) = base(&p.ctx) else {
        return false;
    };
    let code = why.code();
    let now = Instant::now();
    let (f, fresh) = p
        .reg
        .fails()
        .record(r, (code, why.text()), b, cap(&p.ctx, b), now);
    if is_link(code) {
        if p.reg.fails().node_down() {
            let c = code.map(|c| format!("{c:?}")).unwrap_or_default();
            let msg = format!(
                "узел BSC не отвечает или ограничил запросы ({c}): {} — пулы, которые не прочитались, недостоверны \
                 и читаются снова не раньше чем через {b} мс (пауза не растёт: это сбой узла, а не пула); сессии пачек \
                 не рвутся (решение 339). Первый пул: {title} {ticker} ({})",
                why.text(),
                label(r)
            );
            p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
        }
    } else if fresh {
        let msg = format!(
            "{title} {ticker} ({}): состояние пула не прочитано: {} — пул недостоверен (в котировки и сделки не \
             идёт), следующее чтение не раньше чем через {} мс (пауза растёт с каждой неудачей); остальные пулы пачки \
             работают без переподключения (решение 339)",
            label(r),
            why.text(),
            f.pause_ms
        );
        p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
    }
    alert(p, r, title, ticker, &f, now).await;
    true
}

/// Что делает: Н7 — пул не читается дольше dex.pool_unread_alert_s: одно уведомление на эпизод (журнал и ядру).
/// Принимает: окружение, ссылку, вид, тикер, отметку, «сейчас». Ошибок нет (канал ядра закрыт — запись в журнал).
async fn alert(p: &Pools, r: &PoolRef, title: &str, ticker: &str, f: &Fail, now: Instant) {
    let Some(limit) = alert_after(&p.ctx) else {
        return;
    };
    let secs = now.saturating_duration_since(f.since).as_secs();
    if f.told || secs < limit || !p.reg.fails().tell(r) {
        return;
    }
    let text = format!(
        "{title} {ticker} ({}): пул не читается {secs} с (порог dex.pool_unread_alert_s = {limit} с), неудач подряд \
         {}. Последняя причина: {}. Пул недостоверен — в котировки и сделки не идёт; бот читает его снова сам (не \
         раньше паузы {} мс)",
        label(r),
        f.tries,
        f.why,
        f.pause_ms
    );
    p.ctx.log(Level::Warn, "dex.pool", text.clone(), None).await;
    let (out, ctx) = (p.out.clone(), p.ctx.clone());
    tokio::spawn(async move {
        let ev = MarketEvent::Notice {
            venue: ctx.venue(),
            code: CODE_UNREAD.to_string(),
            text,
        };
        if out.send(ev).await.is_err() {
            let msg = format!("Уведомление {CODE_UNREAD} не ушло — канал событий ядру закрыт");
            ctx.log(Level::Warn, "dex.pool", msg, None).await;
        }
    });
}

/// Что делает: пул «каждым блоком» (per_block*.rs) не прочитался: недостоверен (как раньше) и, при изоляции, отметка
/// с причиной и паузой (следующие блоки его не читают, пока пауза не прошла). Принимает: окружение, пул, причину.
/// Ошибок нет.
pub async fn block_failed<R: Reason + ?Sized>(p: &Pools, f: &Found, why: &R) {
    p.reg.update(&f.reference, |e| e.valid = false);
    let ticker = p
        .reg
        .get(&f.reference)
        .map(|e| e.ticker)
        .unwrap_or_default();
    note(p, &f.reference, f.family.title, &ticker, why).await;
}

/// Что делает: пул `i` пачки — в очередь повтора (Б1, см. шапку). Принимает: включена ли изоляция (pool_fail::base),
/// ждущие, брошенные, индекс. Ошибок нет.
pub fn requeue(
    isolate: bool,
    missing: &mut Vec<Missing>,
    abandoned: &mut Vec<Abandoned>,
    i: usize,
) {
    if isolate && abandoned.iter().any(|a| a.i == i && a.setting.is_some()) {
        return;
    }
    super::stale_read::requeue(missing, abandoned, i);
}
