//! Файл: venues/bsc_dex/pools/aero_mev.rs
//! Что делает: решение 470 (T451-01/02, T470-02) — MEV-налог Aero для нашего обмена по формуле модуля
//! PriorityFeeMevTaxModule (Dromos-Labs/metadex-public 1.1.0, V3/src/fees/PriorityFeeMevTaxModule.sol L90-L106):
//! превышение = tx.gasprice − basefee; превышение ≤ max(baseFeeFactor·basefee, minThreshold) — 0; иначе
//! min(mulDivUp(превышение − порог, priorityFeeMultiplier, 1e12), 100000) пипсов (1e6).
//! tx.gasprice в Nitro (факт исходника): опкод GASPRICE = GetPaidGasPrice — при выключенном сборе чаевых = basefee,
//! при включённом — цена газа транзакции min(maxFee, basefee + чаевые) (OffchainLabs/nitro ccc5c828
//! arbos/tx_processor.go L995-L1035; OffchainLabs/go-ethereum 260bc5dc core/state_transition.go L396-L400, L660-L663).
//! Сбор чаевых: ArbOS 9 — да (tx_processor.go L1003-L1005); иные < 60 — нет; ≥ 60 — ArbOwnerPublic.getCollectTips()
//! (tx_processor.go L1006-L1015;
//! ArbSys.arbOSVersion() = 55 + версия, precompiles/ArbSys.go L67-L69). Значит: сбор выключен — налог 0 всегда.
//! Включён — налог по наибольшим чаевым режима (fixed — send.priority_fee_gwei, profit_share — send.tip_cap_gwei) и
//! наименьшему порогу minThreshold (baseFeeFactor·basefee следующего блока неизвестен) — верхняя оценка.
//! В Base OP Stack и Ethereum тип 2 оплачивает приоритетную комиссию. Их расчёт использует предел tip без ArbSys.
//! Решение 470 (ревью t240): модуль MEV-налога закреплён настройкой dex.aero_mev_module (pinned): у фабрики Basic и у
//! крючка Slipstream V3 текущий модуль читается при каждом чтении; не совпал — пул не торгуется (формула сверена только
//! для этого кода модуля).
//! Кто вызывает: pools/load_aero_basic.rs, pools/aero_cl.rs, pools/per_block_aero.rs; exec/solidly_gate.rs (fresh —
//! перед отправкой; решение 470, T470-06: у исполнителя v5 налог не мешает — шаг AERO_BASIC).
//! От чего зависит: node/call.rs, send/gas_mode.rs, send/tip_mode.rs, pools/abi_aero_basic.rs.
//! Настройки: dex.aero_mev_module [DEX]; dex.arb_sys, dex.arb_owner_public, dex.gas_mode [площадка DEX]; send.tip_mode,
//! send.priority_fee_gwei, send.tip_cap_gwei [площадка DEX].

use alloy_primitives::{Address, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::super::send::gas_mode::{self, GasMode};
use super::super::send::tip_mode::{self, TipMode};
use super::abi_aero_basic::{IAeroBasicFactory, IAeroMevTaxModule, IArbOwnerPublic, IArbSys};

/// MEV_TAX_CAP модуля (PriorityFeeMevTaxModule.sol L22) и PRECISION (L25) — константы кода модуля.
const MEV_TAX_CAP: u32 = 100_000;
const PRECISION: u64 = 1_000_000_000_000;
/// ArbSys.arbOSVersion() = 55 + версия ArbOS (precompiles/ArbSys.go L68); сбор чаевых — ArbOS 9 всегда, с ArbOS 60 —
/// по флагу (tx_processor.go L1002-L1015; params.ArbosVersion_9 / ArbosVersion_60).
const ARBOS_OFFSET: u64 = 55;
const ARBOS_V9: u64 = 9;
const ARBOS_TIPS: u64 = 60;

/// Наибольшие чаевые режима, wei за газ: fixed — сами чаевые, profit_share — верхний предел. Ошибок нет.
pub fn max_tip(m: &TipMode) -> u128 {
    match m {
        TipMode::Fixed(t) => *t,
        TipMode::ProfitShare(c) => c.cap,
    }
}

/// Налог модуля, пипсы (1e6), — L96-L103 при пороге `threshold`. Принимает: превышение цены газа над basefee, порог,
/// множитель. Ошибок нет.
pub fn tax_pips(over: u128, threshold: U256, mult: u64) -> u32 {
    let over = U256::from(over);
    if over <= threshold {
        return 0;
    }
    let num = (over - threshold).saturating_mul(U256::from(mult));
    let up = num.div_ceil(U256::from(PRECISION));
    up.min(U256::from(MEV_TAX_CAP)).to::<u32>()
}

/// Сбор чаевых сети включён ли (см. шапку). Ошибки: не заданы dex.arb_sys / dex.arb_owner_public; узел.
async fn collect_tips(ctx: &Ctx, tag: &str) -> GatewayResult<bool> {
    let sys = ctx.addr("dex.arb_sys")?;
    let v = call::view(ctx, sys, &IArbSys::arbOSVersionCall {}, tag).await?;
    // Ревью t240: ArbOS 9 собирает чаевые всегда (tx_processor.go L1003-L1005), остальные ниже 60 — нет.
    if v == U256::from(ARBOS_OFFSET + ARBOS_V9) {
        return Ok(true);
    }
    if v < U256::from(ARBOS_OFFSET + ARBOS_TIPS) {
        return Ok(false);
    }
    let owner = ctx.addr("dex.arb_owner_public")?;
    call::view(ctx, owner, &IArbOwnerPublic::getCollectTipsCall {}, tag).await
}

/// Налог нашего обмена (верхняя оценка), пипсы. Принимает: окружение, модуль (нулевой — 0), тег. Ошибки:
/// NotApplicable — модель газа не eip1559_l1 при включённом сборе чаевых; не заданы настройки режима чаевых и
/// прекомпилей; узел, ответ не по ABI (у модуля нет priorityFeeMultiplier/minThreshold).
pub async fn tax(ctx: &Ctx, module: Address, tag: &str) -> GatewayResult<u32> {
    if module.is_zero() {
        return Ok(0);
    }
    let ask = IAeroMevTaxModule::priorityFeeMultiplierCall {};
    let mult = call::view(ctx, module, &ask, tag).await?;
    if mult == 0 {
        return Ok(0);
    }
    match gas_mode::read(ctx)? {
        GasMode::Eip1559L1 => {
            if !collect_tips(ctx, tag).await? {
                return Ok(0);
            }
        }
        GasMode::Ethereum | GasMode::OpStack => {}
        GasMode::Legacy => {
            return Err(ctx.err(
                ErrorCode::NotApplicable,
                "Модуль MEV Aero требует подтверждённую модель приоритетной комиссии сети",
            ));
        }
    }
    let tip = max_tip(&tip_mode::read(ctx)?);
    let min = call::view(ctx, module, &IAeroMevTaxModule::minThresholdCall {}, tag).await?;
    Ok(tax_pips(tip, U256::from(min.to::<u128>()), mult))
}

/// Решение 470 (ревью t240): текущий модуль MEV-налога = закреплённому dex.aero_mev_module DEX `id`. Принимает:
/// окружение, id DEX, прочитанный модуль, что читали (текст причины). Ошибки: не задана dex.aero_mev_module;
/// NotApplicable — модуль другой (пул не торгуется: формула налога сверена только для закреплённого кода).
pub fn pinned(ctx: &Ctx, id: &str, module: Address, what: &str) -> GatewayResult<()> {
    let want = ctx.dex_addr(MEV_MODULE, id)?;
    if module == want {
        return Ok(());
    }
    Err(ctx.err(
        ErrorCode::NotApplicable,
        format!("{id}: модуль MEV-налога {what} {module} ≠ {MEV_MODULE} {want} — пул не торгуется"),
    ))
}

/// Настройка закреплённого модуля MEV-налога (по DEX).
pub const MEV_MODULE: &str = "dex.aero_mev_module";

/// Налог фабрики Basic в б.п.: min(divUp(налог, 100), MAX_FEE − база) — PoolFactory.sol L303 (MAX_FEE = 2000, L36).
/// Принимает: налог (пипсы), базовую комиссию (б.п.). Ошибок нет.
pub fn basic_bps(tax: u32, base: u32) -> u32 {
    tax.div_ceil(100).min(2000u32.saturating_sub(base))
}

/// Перед отправкой: пауза фабрики Aero Basic и налог её текущего модуля на последнем блоке. Налог > 0 при шаге SOLIDLY —
/// отказ: SOLIDLY считает выход getAmountOut без налога, своп откатился бы по K (Pool.sol L172, L193). Решение 470
/// (T470-06): у исполнителя v5 шаг AERO_BASIC берёт выход getAmountOutWithTotalFee (налог этой транзакции,
/// L548-L555) — налог не мешает (`taxed_ok`), проверяется пауза и закреплённый модуль. Принимает: окружение, id DEX,
/// фабрику, исполнит ли шаг налог (exec_kind::for_wallet = AERO_BASIC). Ошибки: NoRoute — пауза (Pool.sol L144) или
/// налог при SOLIDLY; NotApplicable — модуль не закреплённый (pinned); как tax.
pub async fn fresh(ctx: &Ctx, id: &str, factory: Address, taxed_ok: bool) -> GatewayResult<()> {
    let fee_module =
        call::view(ctx, factory, &IAeroBasicFactory::feeModuleCall {}, "latest").await?;
    let setting = super::load_aero_basic::FEE_MODULE;
    if fee_module != ctx.dex_addr(setting, id)? {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            format!(
                "{id}: изменился модуль комиссии фабрики {factory}: {fee_module}, сверить {setting}"
            ),
        ));
    }
    if call::view(ctx, factory, &IAeroBasicFactory::isPausedCall {}, "latest").await? {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "фабрика Aero Basic на паузе (isPaused) — обмен откатился бы",
        ));
    }
    let ask = IAeroBasicFactory::mevTaxModuleCall {};
    let module = call::view(ctx, factory, &ask, "latest").await?;
    pinned(ctx, id, module, "фабрики")?;
    if taxed_ok {
        return Ok(());
    }
    match tax(ctx, module, "latest").await? {
        0 => Ok(()),
        t => Err(ctx.err(
            ErrorCode::NoRoute,
            format!(
                "Aero Basic: MEV-налог {t} пипс — шаг SOLIDLY откатился бы; шаг AERO_BASIC — у исполнителя v5 \
                 (T470-06)"
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Формула L96-L103 на параметрах конструктора volatile-модуля Arbitrum (pools-arbitrum.json: множитель 1e9,
    /// порог 1e9 wei): на пороге — 0; +1e12 wei сверх порога — 1e12·1e9/1e12 = 1e9 → предел 100000; +1000 wei —
    /// mulDivUp(1000, 1e9, 1e12) = 1.
    #[test]
    fn formula() {
        let thr = U256::from(1_000_000_000u64);
        assert_eq!(tax_pips(1_000_000_000, thr, 1_000_000_000), 0);
        assert_eq!(
            tax_pips(1_000_000_000 + 1_000_000_000_000, thr, 1_000_000_000),
            100_000
        );
        assert_eq!(tax_pips(1_000_000_000 + 1000, thr, 1_000_000_000), 1);
        assert_eq!(tax_pips(u128::MAX, thr, 0), 0);
        assert_eq!(basic_bps(1, 30), 1);
        assert_eq!(basic_bps(100_000, 1990), 10);
    }
}
