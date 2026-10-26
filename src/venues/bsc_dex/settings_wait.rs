//! Файл: venues/bsc_dex/settings_wait.rs
//! Что делает: «настройка не задана — задали на лету, работа пошла без перезапуска» (CLAUDE.md: всё меняется на лету
//! без перезапуска). Цикл, которому нужна настройка (интервал прохода, пауза переподключения, опрос блоков), не
//! останавливается навсегда, а ждёт изменения настроек и пробует снова (until_ok). Итоги проверок с причиной «не задана
//! настройка» (налоги — Why::Config, сверка с квотером, крючки) перепроверяются при изменении настроек, а не через
//! полный проход (is_config — по тексту причины, как он приходит из хранилища настроек).
//! Кто вызывает: check/{tax_cycle, quoter_cycle, cycle}.rs, node/heads.rs, pools/listen.rs.
//! От чего зависит: settings/live.rs (Watch), ctx.rs.
//! Настройки: нет (ждёт любые).

use crate::gateway::errors::GatewayResult;
use crate::settings::live::Watch;
use crate::storage::journal::Level;

use super::ctx::Ctx;

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

/// Значение из `read`, а пока его нет (не задано или задано неверно) — одна запись в журнал «`what` ждёт настройку:
/// причина» и ожидание изменения настроек, затем снова; дождались после ожидания — запись «`what` снова работает».
/// Принимает: окружение, что ждёт (для журнала), вид записи журнала, чтение. Возвращает: значение; None — рассылка
/// настроек закрыта (система останавливается). Ошибок нет.
pub async fn until_ok<T>(
    ctx: &Ctx,
    what: &str,
    kind: &str,
    read: impl Fn() -> GatewayResult<T>,
) -> Option<T> {
    let mut waited = false;
    loop {
        // Подписка — до чтения: изменение между чтением и ожиданием не теряется.
        let mut w = watch(ctx);
        match read() {
            Ok(v) => {
                if waited {
                    let msg = format!("{what}: настройку задали — снова работает");
                    ctx.log(Level::Info, kind, msg, None).await;
                }
                return Some(v);
            }
            Err(e) => {
                if !waited {
                    let msg = format!(
                        "{what} ждёт настройку: {} — продолжит сам, как только её зададут",
                        e.message
                    );
                    ctx.log(Level::Error, kind, msg, None).await;
                    waited = true;
                }
                w.next().await?;
            }
        }
    }
}
