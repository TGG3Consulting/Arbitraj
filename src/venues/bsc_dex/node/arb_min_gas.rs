//! Файл: venues/bsc_dex/node/arb_min_gas.rs
//! Что делает: решение 436 (Б) — нижний предел цены газа сети Arbitrum с узла: getMinimumGasPrice() прекомпиля
//! ArbGasInfo через eth_call. Адрес прекомпиля — настройка площадки dex.arb_gas_info (не код, как dex.node_interface
//! решения 433). Значение читается вместе с baseFee головы (quote/gas.rs fetch при dex.gas_mode = eip1559_l1 — раз на
//! блок, в котором нужна цена газа; прогрев quote/warm.rs — на каждый новый блок) и хранится по площадке; его берёт
//! минимум канала «Секвенсор Arbitrum» вместо channel.min_gas_price_gwei (send/ready.rs). Нового опроса нет.
//! Кто вызывает: quote/gas.rs (refresh), send/ready.rs (floor), send/check.rs и connection_gas.rs (показ).
//! От чего зависит: node/call.rs (eth_call и разбор по ABI), ctx.rs, alloy sol!.
//! Настройки: dex.arb_gas_info [площадка DEX].
//!
//! Источники (клоны: OffchainLabs/nitro ccc5c828d9ea22f161efc784bac4d4a6559b7e59, OffchainLabs/go-ethereum
//! 260bc5dcbd5a16925d7e26145577b2612f30f428, OffchainLabs/arbitrum-docs 5ce1a9e1c75fa55e031c0be57baa8f7989abf471):
//! - метод: nitro precompiles/ArbGasInfo.go:175-178 — «GetMinimumGasPrice gets the minimum gas price needed for a
//!   transaction to succeed», возвращает L2PricingState().MinBaseFeeWei() (тип huge = uint256);
//! - сигнатура: arbitrum-docs content/docs/launch-arbitrum-chain/chain-config/costs/fee-management.mdx:37,40
//!   («getMinimumGasPrice()(uint256)», cast call на 0x…6C); таблица прекомпиля —
//!   content/partials/precompile-tables/_ArbGasInfo.mdx:111 (интерфейс nitro-precompile-interfaces ArbGasInfo.sol#L64);
//! - адрес: go-ethereum core/types/arbitrum_signer.go:18 (ArbGasInfoAddress = 0x6c), регистрация — nitro
//!   precompiles/precompile.go:528; docs — content/partials/_reference-arbitrum-contract-addresses-partial.mdx:85
//!   (0x000000000000000000000000000000000000006C у Arbitrum One 42161);
//! - смысл: базовая цена блока не ниже этого минимума — nitro arbos/l2pricing/model.go:204-209 (baseFee = minBaseFee ×
//!   exp(…) ≥ minBaseFee); у проверки транзакции перед секвенсором maxFeePerGas ниже минимума — ErrFeeCapTooLow
//!   (execution/gethexec/tx_pre_checker.go:153-161); меняет минимум владелец сети — ArbOwner.SetMinimumL2BaseFee
//!   (precompiles/ArbOwner.go:252); docs.arbitrum.io «Gas and fees» (content/docs/how-arbitrum-works/deep-dives/
//!   gas-and-fees.mdx:181: «has a set floor, which can be queried via ArbGasInfo's getMinimumGasPrice»).
//!
//! Записи ответа своего узла нет — тест разбора не написан (docs/TASKS.md, «Решение 436»: нужна запись).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use alloy_primitives::U256;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::call;

alloy::sol! {
    /// Прекомпиль ArbGasInfo Nitro: нижний предел цены газа сети (nitro precompiles/ArbGasInfo.go:175-178).
    interface IArbGasInfo {
        function getMinimumGasPrice() external view returns (uint256);
    }
}

/// Площадка DEX (ctx.place.id) → (блок, минимум wei). Площадки — разные сети.
type Seen = HashMap<&'static str, (u64, U256)>;

/// Последнее прочитанное значение по площадке DEX.
static LAST: LazyLock<Mutex<Seen>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Минимум цены газа сети с узла. Принимает: окружение, блок («latest» или номер в hex — node/call.rs block_tag).
/// Возвращает: wei. Ошибки: NotConfigured — dex.arb_gas_info не задана; адрес не адрес; узел; BadResponse — не по ABI.
pub async fn read(ctx: &Ctx, block: &str) -> GatewayResult<U256> {
    let at = ctx.addr("dex.arb_gas_info")?;
    call::view(ctx, at, &IArbGasInfo::getMinimumGasPriceCall {}, block).await
}

/// Обновляет минимум на блоке цены газа (вызывает quote/gas.rs вместе с baseFee). Принимает: окружение, номер блока.
/// Возвращает: Ok — прочитан (или dex.arb_gas_info не задана: тогда не читается, канал секвенсора не готов с этой
/// причиной — floor). Ошибки: узел, BadResponse, адрес не адрес — цены газа на этом блоке нет (не торгуем вслепую).
pub async fn refresh(ctx: &Ctx, block: u64) -> GatewayResult<()> {
    let v = match read(ctx, &call::block_tag(block)).await {
        Ok(v) => v,
        Err(e) if e.code == ErrorCode::NotConfigured => return Ok(()),
        Err(e) => return Err(e),
    };
    let mut all = LAST.lock().unwrap_or_else(|p| p.into_inner());
    // Запросы могут вернуться не по порядку: более старый блок не затирает новый.
    if all.get(ctx.place.id).is_none_or(|(b, _)| *b <= block) {
        all.insert(ctx.place.id, (block, v));
    }
    Ok(())
}

/// Последнее прочитанное значение площадки: (блок, wei). None — ещё не читалось. Ошибок нет.
pub fn last(ctx: &Ctx) -> Option<(u64, U256)> {
    let all = LAST.lock().unwrap_or_else(|p| p.into_inner());
    all.get(ctx.place.id).copied()
}

/// Минимум канала, который берёт его с узла (send/ready.rs). Принимает: окружение. Возвращает: wei.
/// Ошибки (текст причины для состояния канала): dex.arb_gas_info не задана или не адрес; значение ещё не прочитано
/// (читается вместе с baseFee головы только при dex.gas_mode = eip1559_l1 — у BSC канал секвенсора не готов).
pub fn floor(ctx: &Ctx) -> Result<U256, String> {
    if let Some((_, v)) = last(ctx) {
        return Ok(v);
    }
    ctx.addr("dex.arb_gas_info").map_err(|e| e.message)?;
    let why = "минимальная цена газа сети с узла (ArbGasInfo.getMinimumGasPrice) ещё не прочитана: \
               читается вместе с baseFee головы при dex.gas_mode = eip1559_l1 \
               (котировка, прогрев цены газа)";
    Err(why.into())
}
