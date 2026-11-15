//! Файл: venues/bsc_dex/send/bundle_pick.rs
//! Что делает: решение 299 — каналы для отправки бандла из нескольких транзакций (решение 84) при включённой настройке
//! send.bundle_skip_single: одиночные каналы (channel.method = single) исключаются из ЭТОЙ отправки, бандл уходит только
//! в бандловые; ни одного бандлового — отказ с причиной. Настройка не задана / нет — всё как до решения 299: каналы не
//! фильтруются, любой одиночный среди готовых — отказ (send/mod.rs planned_bundle_ok, dispatch_slice; решение 121 B3).
//! Одиночные сделки (одна транзакция, send/mod.rs dispatch) это не трогает — они идут во все готовые каналы.
//! Атомарность не ослабляется: в бандловый канал уходит весь бандл одним eth_sendBundle без revertingTxHashes
//! (всё-или-ничего, send/ready.rs body), а одиночный канал не получает ни одной транзакции бандла.
//! Кто вызывает: send/mod.rs (planned_bundle_ok), exec/bundle_send.rs (отправка бандла графа и цикла DEX ↔ DEX).
//! От чего зависит: send/ready.rs (Ready), ctx.rs, core/types.rs (SendMethod).
//! Настройки: send.bundle_skip_single [bsc_dex] (необязательная).

use crate::core::types::SendMethod;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::ready::Ready;

/// Что делает: читает send.bundle_skip_single. Возвращает: да — исключать одиночные каналы из отправки бандла;
/// не задано / нет — не исключать (поведение до решения 299). Ошибок нет.
pub fn enabled(ctx: &Ctx) -> bool {
    ctx.flag("send.bundle_skip_single").unwrap_or(false)
}

/// Каналы отправки бандла и исключённые одиночные.
pub struct Picked {
    /// Каналы, в которые уйдёт бандл.
    pub ready: Vec<Ready>,
    /// Названия исключённых одиночных каналов (пусто — никто не исключён).
    pub skipped: Vec<String>,
}

/// Что делает: отбирает каналы для отправки бандла. Принимает: окружение, готовые каналы (send::select).
/// Возвращает: настройка выключена — все каналы как есть, исключённых нет (dispatch_slice откажет при одиночном, как
/// раньше); включена — только бандловые и названия исключённых одиночных. Ошибки: NotConfigured — включена, а
/// бандловых среди готовых нет (бандл некуда отправить).
pub fn pick(ctx: &Ctx, ready: Vec<Ready>) -> GatewayResult<Picked> {
    if !enabled(ctx) {
        return Ok(Picked {
            ready,
            skipped: Vec::new(),
        });
    }
    let (bundle, single): (Vec<Ready>, Vec<Ready>) = ready
        .into_iter()
        .partition(|r| r.method == SendMethod::Bundle);
    let skipped: Vec<String> = single.iter().map(|r| r.ch.title.to_string()).collect();
    if bundle.is_empty() {
        return Err(ctx.err(ErrorCode::NotConfigured, no_bundle_text(&skipped)));
    }
    Ok(Picked {
        ready: bundle,
        skipped,
    })
}

/// Что делает: проверка на котировке при включённой настройке — исполним ли бандл при каналах, готовых сейчас.
/// Принимает: готовые каналы (не пусто — пустоту проверяет вызывающий). Возвращает: Ok — есть хотя бы один бандловый;
/// иначе причина. Ошибок нет.
pub fn planned(ready: &[Ready]) -> Result<(), String> {
    if ready.iter().any(|r| r.method == SendMethod::Bundle) {
        return Ok(());
    }
    let titles: Vec<String> = ready.iter().map(|r| r.ch.title.to_string()).collect();
    Err(no_bundle_text(&titles))
}

/// Что делает: текст отказа «ни одного бандлового канала». Принимает: названия готовых (одиночных) каналов.
/// Возвращает: текст. Ошибок нет.
fn no_bundle_text(single: &[String]) -> String {
    format!(
        "маршрут исполняется только бандлом, одиночные каналы исключаются (send.bundle_skip_single = да), но среди \
         готовых каналов нет ни одного в режиме бандла (channel.method = bundle); одиночные: {} — сигнал, авто-сделкой \
         не исполняется",
        if single.is_empty() {
            "—".to_string()
        } else {
            single.join(", ")
        }
    )
}
