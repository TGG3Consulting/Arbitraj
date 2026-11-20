//! Файл: venues/bsc_dex/send/pool_cond_mode.rs
//! Что делает: решение 439 — режим условия пулов маршрута (knownAccounts) в условной отправке секвенсора Arbitrum
//! для сделки цикла DEX ↔ DEX через исполнитель, настройка send.pool_cond_mode:
//! - `off` — без условия пулов: условная отправка только с окном времени timestampMax (как решение 436);
//! - `pools` — с условием пулов (решение 438, exec/pool_cond_read.rs): значения пулов читаются на блоке плана.
//!
//! Не задана — условия пулов нет, как `off` (стартовых значений нет: не задана — функция не работает). Решение
//! Тиграна 20.11.2026: старт с `off`, по статистике (exec/pool_cond_stat.rs) решить, включать ли `pools`. Значение не
//! из списка — NotConfigured с причиной: вызывающий условие пулов не ставит, причина — на экране статистики.
//! Кто вызывает: exec/pool_cond_read.rs (wanted), exec/pool_cond_stat.rs (view — показ режима).
//! От чего зависит: ctx.rs; варианты — settings/catalog/send.rs POOL_COND_MODES (тест сверяет).
//! Настройки: send.pool_cond_mode [площадка DEX].

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;

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
