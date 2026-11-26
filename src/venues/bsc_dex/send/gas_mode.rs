//! Файл: venues/bsc_dex/send/gas_mode.rs
//! Модель газа задаётся для каждой площадки. Котировка, подключение, расходы цикла и отправка читают одну настройку.
//! BSC — legacy; Arbitrum — type-2 с NodeInterface; Ethereum — type-2; Base — type-2 с GasPriceOracle.
//! https://eips.ethereum.org/EIPS/eip-1559 и https://docs.optimism.io/op-stack/transactions/fees
use crate::gateway::errors::GatewayResult;
use crate::settings::evm_profiles;

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

    pub(crate) fn strict_type2(self) -> bool {
        matches!(self, Self::Ethereum | Self::OpStack)
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
    let id = ctx.int("dex.evm_chain_id").ok();
    if let Some(known) = id.and_then(evm_profiles::legacy_network_mode)
        && mode.as_str() != known
    {
        return Err(ctx.not_set(
            "dex.gas_mode",
            format!("У этой физической сети модель газа {known}"),
        ));
    }
    if let Some(p) =
        evm_profiles::profile(ctx.place.id).or_else(|| id.and_then(evm_profiles::by_chain_id))
    {
        if id != Some(p.evm_chain_id)
            || ctx.text("dex.chain")? != p.chain
            || mode.as_str() != p.gas_mode
        {
            return Err(ctx.not_set(
                "dex.gas_mode",
                "Модель газа, идентификатор EVM и выбранная сеть расходятся",
            ));
        }
    } else if mode.strict_type2() {
        let chain = ctx.text("dex.chain")?;
        if id.is_none_or(|v| v == 0)
            || chain.trim().is_empty()
            || evm_profiles::PROFILES.iter().any(|p| p.chain == chain)
        {
            return Err(ctx.not_set(
                "dex.gas_mode",
                "Для этой модели нужна согласованная идентичность физической сети",
            ));
        }
    }
    Ok(mode)
}

/// Проверка каждого контактного адреса при первом использовании и после изменений.
pub(in crate::venues::bsc_dex) fn endpoint_chain(ctx: &Ctx) -> Option<u64> {
    ctx.chains.transport(ctx)
}

pub(in crate::venues::bsc_dex) fn configured_endpoint(ctx: &Ctx) -> Option<u64> {
    if let Some(p) = evm_profiles::profile(ctx.place.id) {
        return Some(p.evm_chain_id);
    }
    let id = ctx.int("dex.evm_chain_id").ok()?;
    if evm_profiles::by_chain_id(id).is_some() {
        return Some(id);
    }
    ctx.text("dex.gas_mode")
        .ok()
        .and_then(|s| GasMode::parse(&s))
        .filter(|m| m.strict_type2())
        .map(|_| id)
}

/// Известная физическая сеть определяет обязательные поля уже переданной транзакции,
/// даже если настройку газа впоследствии очистили. У других сетей действует их сохранённая модель.
pub(in crate::venues::bsc_dex) fn receipt_needs_l1(ctx: &Ctx) -> bool {
    let Some(id) = endpoint_chain(ctx) else {
        return false;
    };
    if let Some(profile) = evm_profiles::by_chain_id(id) {
        return GasMode::parse(profile.gas_mode) == Some(GasMode::OpStack);
    }
    ctx.text("dex.gas_mode")
        .is_ok_and(|mode| GasMode::parse(&mode) == Some(GasMode::OpStack))
}

/// Старые снимки обычных площадок могли не содержать настройки газа. Вспомогательный
/// расчёт только надбавки в этом случае сохраняет ноль. Для подписи нужен read().
pub(in crate::venues::bsc_dex) fn optional(ctx: &Ctx) -> GatewayResult<Option<GasMode>> {
    if ctx.text("dex.gas_mode").is_err()
        && !evm_profiles::isolated_transport(&ctx.settings, ctx.place.id)
    {
        return Ok(None);
    }
    read(ctx).map(Some)
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
