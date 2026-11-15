//! Файл: venues/bsc_dex/pools/stale_read.rs
//! Что делает: решение 298 — перечитывание пула по событию на блоке события опоздало: у своего узла (не архивного,
//! хранит состояние только последних блоков) состояния этого блока уже нет — ответ −32000 «historical state … is not
//! available» / «missing trie node» (errors.rs → StaleData). Такая ошибка НЕ рвёт сессию пачки: пул уже отмечен
//! недостоверным (listen_log.rs — в котировки и сделки не идёт), одна запись в журнал, пул встаёт в очередь повторного
//! чтения пачки (Members.missing) и перечитывается на новых блоках существующим механизмом (reload.rs retry —
//! на голове, не раньше; события до блока чтения пропускаются, registry.rs has_applied). Прочие ошибки чтения — как
//! раньше: конец сессии пачки.
//! Кто вызывает: pools/listen_log.rs (is_stale, note), pools/session.rs (requeue).
//! От чего зависит: gateway/errors.rs (ErrorCode::StaleData), reload.rs (Missing), revive.rs (Abandoned), registry.rs.
//! Настроек нет (повтор — dex.pool_load_retries через reload.rs).

use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::storage::journal::Level;

use super::listen::Pools;
use super::registry::Entry;
use super::reload::Missing;
use super::revive::Abandoned;

/// Что делает: ошибка чтения — «данные на этом блоке недоступны / устарели» (лечится чтением на более свежем блоке).
/// Принимает: ошибку шлюза. Возвращает: true — StaleData (у узла нет состояния блока, errors.rs). Ошибок нет.
pub fn is_stale(e: &GatewayError) -> bool {
    e.code == ErrorCode::StaleData
}

/// Что делает: запись в журнал — пул не перечитан на блоке события, недостоверен, повтор на новых блоках без обрыва
/// подписки. Принимает: окружение, запись пула, блок события, ошибку узла. Ошибок нет.
pub async fn note(p: &Pools, e: &Entry, block: u64, why: &GatewayError) {
    let msg = format!(
        "{} {}: перечитать пул на блоке события {block} не удалось — у узла BSC уже нет состояния этого блока \
         (узел не архивный). Пул недостоверен (в котировки и сделки не идёт) и перечитывается на новых блоках; \
         подписка пачки не прерывается. Ответ узла: {}",
        e.family.title, e.ticker, why.message
    );
    p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
}

/// Что делает: пул `i` пачки — в очередь повторного чтения (попытки заново). Уже ждёт — без второй записи; был брошен
/// (попытки исчерпаны) — снят из брошенных, чтобы не числиться дважды. Принимает: ждущие, брошенные, индекс пула.
/// Ошибок нет.
pub fn requeue(missing: &mut Vec<Missing>, abandoned: &mut Vec<Abandoned>, i: usize) {
    abandoned.retain(|a| a.i != i);
    if !missing.iter().any(|m| m.i == i) {
        missing.push(Missing { i, tries: 0 });
    }
}
