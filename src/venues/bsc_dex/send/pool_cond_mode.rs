//! Файл: venues/bsc_dex/send/pool_cond_mode.rs
//! Что делает: решение 439 — режим условия пулов маршрута (knownAccounts) в условной отправке секвенсора Arbitrum
//! для сделки цикла DEX ↔ DEX через исполнитель, настройка send.pool_cond_mode:
//! - `off` — без условия пулов: условная отправка только с окном времени timestampMax (как решение 436);
//! - `pools` — с условием пулов (решение 438, exec/pool_cond_read.rs): значения пулов читаются на блоке плана.
//!
//! Не задана — условия пулов нет, как `off` (стартовых значений нет: не задана — функция не работает). Решение
//! Тиграна 20.11.2026: старт с `off`, по статистике (exec/pool_cond_stat.rs) решить, включать ли `pools`. Значение не
//! из списка — NotConfigured с причиной: вызывающий условие пулов не ставит, причина — на экране статистики.
//! wanted — условие пулов нужно этой сделке: режим pools, dex.gas_mode = eip1559_l1 (ревью 438 (F)) и включённый канал
//! sequencer с окном channel.cond_time_window_s.
//! Ревью 438 (E): multi_note — режим pools при нескольких включённых каналах (на экране статистики).
//! Кто вызывает: exec/cycle_trade.rs (wanted), exec/pool_cond_stat.rs (view — показ режима, multi_note).
//! От чего зависит: send/{channels, channel, gas_mode, sequencer_cond}.rs, ctx.rs; варианты — settings/catalog/send.rs
//! POOL_COND_MODES (тест сверяет).
//! Настройки: send.pool_cond_mode, dex.gas_mode [площадка DEX]; channel.enabled, channel.cond_time_window_s [канал
//! sequencer].

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::settings::scope::Scope;

use super::super::ctx::Ctx;
use super::channel::Single;
use super::gas_mode::{self, GasMode};
use super::{channels, sequencer_cond};

/// Режим условия пулов.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolCondMode {
    /// Без условия пулов (и когда настройка не задана).
    Off,
    /// С условием пулов маршрута (решение 438).
    Pools,
}

impl PoolCondMode {
    /// Текст режима (как в настройке). Ошибок нет.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Pools => "pools",
        }
    }
}

/// Читает режим. Принимает: окружение. Возвращает: режим; не задана — Off.
/// Ошибки: NotConfigured — значение не из списка (off или pools); Internal хранилища.
pub fn read(ctx: &Ctx) -> GatewayResult<PoolCondMode> {
    let m = match ctx.text("send.pool_cond_mode") {
        Ok(m) => m,
        Err(e) if e.code == ErrorCode::NotConfigured => return Ok(PoolCondMode::Off),
        Err(e) => return Err(e),
    };
    match m.trim() {
        "off" => Ok(PoolCondMode::Off),
        "pools" => Ok(PoolCondMode::Pools),
        other => {
            let why = format!("send.pool_cond_mode = «{other}»: нет режима (off или pools)");
            Err(ctx.not_set("send.pool_cond_mode", why))
        }
    }
}

/// Условие пулов нужно: send.pool_cond_mode = pools (решение 439; не задана, off или не разобрана — нет),
/// dex.gas_mode = eip1559_l1 (ревью 438 (F)) и у площадки есть включённый канал sequencer с окном
/// channel.cond_time_window_s. Ошибок нет (настройка не читается — нет).
pub fn wanted(ctx: &Ctx) -> bool {
    if !matches!(read(ctx), Ok(PoolCondMode::Pools))
        || !matches!(gas_mode::read(ctx), Ok(GasMode::Eip1559L1))
    {
        return false;
    }
    channels::of(ctx)
        .filter(|c| c.single == Some(Single::Sequencer))
        .any(|c| {
            let s = Scope::Channel(c.id.to_string());
            ctx.settings.flag_in("channel.enabled", &s).unwrap_or(false)
                && matches!(sequencer_cond::window(ctx, &s), Ok(Some(_)))
        })
}

/// Ревью 438 (E): режим pools, а включённых каналов площадки больше одного — knownAccounts уйдут только в канал
/// sequencer, в остальные та же транзакция уйдёт без условия. Принимает: окружение. Возвращает: причина для экрана
/// (блок статистики) или None. Ошибок нет.
pub fn multi_note(ctx: &Ctx) -> Option<String> {
    if !matches!(read(ctx), Ok(PoolCondMode::Pools)) {
        return None;
    }
    let on: Vec<&str> = channels::of(ctx)
        .filter(|c| {
            let s = Scope::Channel(c.id.to_string());
            ctx.settings.flag_in("channel.enabled", &s).unwrap_or(false)
        })
        .map(|c| c.title)
        .collect();
    if on.len() < 2 {
        return None;
    }
    Some(format!(
        "условие пулов действует только при единственном канале sequencer: включено {} \
         каналов ({}) — при отправке в несколько каналов транзакция уходит в остальные без условия",
        on.len(),
        on.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use crate::settings::catalog::send::POOL_COND_MODES;

    use super::PoolCondMode;

    /// Варианты каталога — ровно те, что разбирает read.
    #[test]
    fn catalog_and_code_agree() {
        let ours = [PoolCondMode::Off.as_str(), PoolCondMode::Pools.as_str()];
        assert_eq!(POOL_COND_MODES, &ours);
    }
}
