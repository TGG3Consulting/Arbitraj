//! Файл: venues/bsc_dex/settings_wait.rs
//! Что делает: «настройка не задана — задали на лету, работа пошла без перезапуска» (CLAUDE.md: всё меняется на лету
//! без перезапуска). Цикл, которому нужна настройка (интервал прохода, пауза переподключения, опрос блоков), не
//! останавливается навсегда, а ждёт изменения настроек и пробует снова (until_ok). Итоги проверок с причиной «не задана
//! настройка» (налоги — Why::Config, сверка с квотером, крючки) перепроверяются при изменении настроек, а не через
//! полный проход (is_config — по тексту причины, как он приходит из хранилища настроек).
//! Кто вызывает: check/{tax_cycle, quoter_cycle, cycle}.rs, node/heads.rs, pools/listen.rs.
//! От чего зависит: settings/wait.rs (until_ok — общий механизм), settings/live.rs (Watch), ctx.rs.
//! Настройки: нет (ждёт любые).

use crate::gateway::errors::GatewayResult;
use crate::settings::live::Watch;
use crate::settings::wait::Note;

use super::ctx::{Ctx, ID};

/// Начало причины «не задана настройка» (settings/read.rs not_set: «Не задана настройка {имя} — задайте её в
/// интерфейсе»).
pub const NOT_SET: &str = "Не задана настройка";

/// Причина — «не задана настройка» (в том числе внутри составной причины). Ошибок нет.
pub fn is_config(why: &str) -> bool {
    why.contains(NOT_SET)
}

/// Подписка на любые изменения настроек. Ошибок нет.
pub fn watch(ctx: &Ctx) -> Watch {
    Watch::new(ctx.settings.subscribe(), Vec::new())
}

/// Значение настройки, а пока её нет — ожидание (settings/wait.rs until_ok; журнал площадки bsc_dex). Принимает:
/// окружение, что ждёт, вид записи журнала, чтение. Возвращает: значение; None — рассылка настроек закрыта. Ошибок нет.
pub async fn until_ok<T>(
    ctx: &Ctx,
    what: &str,
    kind: &str,
    read: impl Fn() -> GatewayResult<T>,
) -> Option<T> {
    let note = Note {
        journal: &ctx.journal,
        kind,
        venue: Some(ID),
    };
    crate::settings::wait::until_ok(&ctx.settings, note, what, read).await
}
