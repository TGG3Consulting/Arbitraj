//! Файл: venues/bsc_dex/pools/history_gate.rs
//! Что делает: пауза чтения истории пулов V4 из интерфейса (dex.history_scan): выключено — чтение останавливается
//! перед следующим куском (место сохранено в SQLite, уже найденные пулы слушаются, виды пулов не выключаются) и ждёт
//! включения без опроса — по событию изменения настройки. Так на бесплатном тарифе узла экономятся CU (eth_getLogs).
//! Не задана — чтение не идёт (как выключено), с записью в журнал.
//!
//! Кто вызывает: pools/history_v4.rs (перед каждым куском).
//! От чего зависит: ctx.rs, settings/live.rs (Watch), settings/store.rs (subscribe).
//! Настройки: dex.history_scan [bsc_dex].

use crate::settings::live::Watch;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;

/// Ждёт, пока чтение истории разрешено. Принимает: подпись вида пула для журнала. Возвращает сразу, если разрешено;
/// иначе — одна запись «на паузе» и ожидание включения. false — поток изменений настроек закрыт. Ошибок нет.
pub async fn allowed(ctx: &Ctx, what: &str) -> bool {
    let mut w = Watch::new(ctx.settings.subscribe(), vec!["dex.history_scan".into()]);
    let mut told = false;
    loop {
        match ctx.flag("dex.history_scan") {
            Ok(true) => {
                if told {
                    let msg = format!("{what}: чтение истории продолжено (dex.history_scan)");
                    ctx.log(Level::Info, "dex.scan", msg, None).await;
                }
                return true;
            }
            Ok(false) | Err(_) if told => {}
            res => {
                told = true;
                let why = match res {
                    Err(e) => e.message,
                    _ => "выключено (dex.history_scan)".into(),
                };
                let msg = format!("{what}: чтение истории на паузе — {why}");
                ctx.log(Level::Info, "dex.scan", msg, None).await;
            }
        }
        if w.next().await.is_none() {
            return false;
        }
    }
}
