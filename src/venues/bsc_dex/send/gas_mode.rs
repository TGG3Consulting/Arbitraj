//! Файл: venues/bsc_dex/send/gas_mode.rs
//! Модель газа задаётся для каждой площадки. Котировка, подключение, расходы цикла и отправка читают одну настройку.
//! BSC — legacy; Arbitrum — type-2 с NodeInterface; Ethereum — type-2; Base — type-2 с GasPriceOracle.
//! https://eips.ethereum.org/EIPS/eip-1559 и https://docs.optimism.io/op-stack/transactions/fees
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;

/// Модель газа сети площадки DEX.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GasMode {
    /// Одна цена gasPrice, legacy-транзакция с chainId (EIP-155) — BSC.
    Legacy,
    /// EIP-1559 (тип 2) и газ с частью L1 из NodeInterface — Arbitrum.
    Eip1559L1,
    /// Ethereum L1: обычный type-2 без надбавок rollup.
    Ethereum,
    /// Base OP Stack: type-2, L1 data fee и operator fee.
    OpStack,
}

impl GasMode {
    /// Вариант по тексту настройки. Принимает: значение dex.gas_mode. Возвращает: модель или None (неизвестный текст).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "legacy" => Some(Self::Legacy),
            "ethereum_eip1559" => Some(Self::Ethereum),
            "op_stack_eip1559" => Some(Self::OpStack),
            "eip1559_l1" => Some(Self::Eip1559L1),
            _ => None,
        }
    }

    /// Текст варианта (как в настройке). Ошибок нет.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Ethereum => "ethereum_eip1559",
            Self::OpStack => "op_stack_eip1559",
            Self::Eip1559L1 => "eip1559_l1",
        }
    }
}

/// Модель газа площадки. Принимает: окружение. Возвращает: модель.
/// Ошибки: NotConfigured — dex.gas_mode не задана или значение не из списка.
pub fn read(ctx: &Ctx) -> GatewayResult<GasMode> {
    let s = ctx.text("dex.gas_mode")?;
    let mode = GasMode::parse(&s).ok_or_else(|| {
        ctx.not_set(
            "dex.gas_mode",
            format!("dex.gas_mode = «{s}»: неизвестная модель газа"),
        )
    })?;
    let compatible = match (ctx.place.id, mode) {
        ("base_dex", GasMode::OpStack) | ("eth_dex", GasMode::Ethereum) => true,
        ("base_dex" | "eth_dex", _) | (_, GasMode::Ethereum | GasMode::OpStack) => false,
        _ => true,
    };
    if !compatible {
        return Err(ctx.not_set(
            "dex.gas_mode",
            format!("модель {s} не подходит площадке {}", ctx.place.id),
        ));
    }
    Ok(mode)
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
        assert_eq!(GAS_MODES.len(), 4);
        assert_eq!(GasMode::parse("eip1559"), None);
    }
}
