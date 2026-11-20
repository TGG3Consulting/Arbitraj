//! Файл: venues/bsc_dex/send/gas_mode.rs
//! Что делает: решение 433 (шаг 5 двух сетей) — модель газа сети площадки DEX, настройка dex.gas_mode:
//! `legacy` — BSC как сейчас: одна цена gasPrice, legacy-транзакция (send/tx.rs; база газа BSC всегда 0);
//! `eip1559_l1` — Arbitrum One: транзакция EIP-1559 (тип 2, send/tx_1559.rs), газ с частью L1 (NodeInterface —
//! node/arb_gas.rs), стоимость = газ × min(maxFeePerGas, baseFee + чаевые).
//! Отправка и котировка пока читают только legacy (BSC без изменений); переключение отправки, отмены, перевода и
//! котировки по режиму — задача 433-д (docs/TASKS.md, «Решение 433»). Сейчас режим читает «Проверить подключение»
//! (connection_gas.rs).
//! Кто вызывает: connection_gas.rs.
//! От чего зависит: ctx.rs; варианты — settings/catalog/dex.rs GAS_MODES (тест сверяет оба списка).
//! Настройки: dex.gas_mode [площадка DEX].
//!
//! Тип 2 — https://eips.ethereum.org/EIPS/eip-1559 ; газ Arbitrum — https://docs.arbitrum.io/how-arbitrum-works/gas-fees .

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;

/// Модель газа сети площадки DEX.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GasMode {
    /// Одна цена gasPrice, legacy-транзакция с chainId (EIP-155) — BSC.
    Legacy,
    /// EIP-1559 (тип 2) и газ с частью L1 из NodeInterface — Arbitrum.
    Eip1559L1,
}

impl GasMode {
    /// Вариант по тексту настройки. Принимает: значение dex.gas_mode. Возвращает: модель или None (неизвестный текст).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "legacy" => Some(Self::Legacy),
            "eip1559_l1" => Some(Self::Eip1559L1),
            _ => None,
        }
    }

    /// Текст варианта (как в настройке). Ошибок нет.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Eip1559L1 => "eip1559_l1",
        }
    }
}

/// Модель газа площадки. Принимает: окружение. Возвращает: модель.
/// Ошибки: NotConfigured — dex.gas_mode не задана или значение не из списка.
pub fn read(ctx: &Ctx) -> GatewayResult<GasMode> {
    let s = ctx.text("dex.gas_mode")?;
    GasMode::parse(&s).ok_or_else(|| {
        ctx.not_set(
            "dex.gas_mode",
            format!("dex.gas_mode = «{s}»: нет такой модели газа (legacy или eip1559_l1)"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::catalog::dex::GAS_MODES;

    /// Каждый вариант каталога разбирается и печатается обратно тем же текстом — списки кода и каталога совпадают.
    #[test]
    fn catalog_and_code_agree() {
        for s in GAS_MODES {
            assert_eq!(GasMode::parse(s).map(GasMode::as_str), Some(*s));
        }
        assert_eq!(GAS_MODES.len(), 2);
        assert_eq!(GasMode::parse("eip1559"), None);
    }
}
