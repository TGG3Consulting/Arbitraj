//! Файл: venues/bsc_dex/pools/fee_override.rs
//! Что делает: решение 274 — SheepDex: комиссия обмена пула V3-форка задаётся ФАБРИКОЙ по адресу получателя. Пул SpePool
//! в каждом шаге свопа берёт `ISpeFactory(factory).swapFee(recipient, fee)`, а фабрика отдаёт `_addressFees[addr]`, если
//! он больше нуля, иначе исходную fee() пула; задаёт его оператор `setFee(address, uint24)` БЕЗ события. Математика бота
//! считает обмен с fee() пула, поэтому пул DEX с dex.v3_fee_source = factory_swap_fee читается (и перед сделкой
//! перечитывается, quote/verify.rs → load_batch) вместе с `swapFee(адрес, 0)` для кошелька окружения и для его
//! исполнителя ArbExecutor — получателей выхода шага (contracts/ArbExecutor.sol `execute`: выход шага — кошельку или
//! исполнителю). Ответ 0 — своей комиссии у адреса нет (setFee требует _fee > 0) и обмен идёт с fee() пула; не 0 — пул
//! не прочитан с причиной: не торгуем по неверной комиссии. Виртуальный квотер получатель — он сам (`address(this)`),
//! своей комиссии у него нет — его ответ тоже с fee() пула.
//! Кто вызывает: pools/load_batch_v2v3.rs (head, parse_head), pools/load_v2v3.rs (v3).
//! От чего зависит: ctx.rs, ctx_wallet.rs (исполнитель), wallet/signer.rs (адрес кошелька), node/call.rs, alloy::sol!.
//! Настройки: dex.v3_fee_source [DEX] (необязательная: не задана или pool — как раньше), dex.factory [DEX].
//!
//! Источник (verified, Sourcify partial_match, solc 0.7.6): пул 0x197880438436c426392570cb4782432ec9135f62, файл
//! SpePool.sol (sha256 c5342e6b…fc40b) — `swapFee(recipient, fee)` L2333 в цикле шагов swap L2290-L2360; фабрика
//! SpeFactory 0x571521f8c16f3c4ed5f2490f19187ba7a5a3cbdf, SpeFactory.sol (sha256 1675508…76f9) — `_addressFees` L2557,
//! `setFee` L2598-L2602 (оператор swapDirector, 0 < fee ≤ 3000), `swapFee` L2604-L2610; интерфейс ISpeFactory L16.
//! https://sourcify.dev/server/files/any/56/0x571521f8c16f3c4ed5f2490f19187ba7a5a3cbdf

use alloy::sol_types::SolCall;
use alloy_primitives::aliases::U24;
use alloy_primitives::{Address, Bytes};

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::super::wallet::signer;
use super::families::{Family, Proto};
use super::load_batch::{Miss, miss};

alloy::sol! {
    /// ISpeFactory (SpeFactory.sol L16): своя комиссия адреса или originalFee.
    interface ISpeFactory {
        function swapFee(address addr, uint24 originalFee) external view returns (uint24 fee);
    }
}

/// Значение dex.v3_fee_source, при котором комиссию адреса проверяет фабрика.
pub const FACTORY: &str = "factory_swap_fee";

/// Нужна ли проверка: вид V3 (ABI Uniswap или PancakeSwap) и dex.v3_fee_source = factory_swap_fee. Не задана — нет
/// (поведение прежнее). Ошибок нет.
pub fn on(ctx: &Ctx, f: &Family) -> bool {
    matches!(f.proto, Proto::UniswapV3 | Proto::PancakeV3)
        && ctx
            .dex_text("dex.v3_fee_source", f.id)
            .is_ok_and(|v| v == FACTORY)
}

/// Получатели выхода шага у этого окружения: кошелёк (счёт сделки или строка «пробы») и его исполнитель, если
/// развёрнут. Ошибки: секреты закрыты / кошелька нет (адрес кошелька — из ключа), адрес исполнителя не адрес.
fn recipients(ctx: &Ctx) -> GatewayResult<Vec<Address>> {
    let mut l = vec![signer::address(ctx)?];
    if let Some(x) = ctx.executor()? {
        l.push(x);
    }
    Ok(l)
}

/// Вызовы `factory.swapFee(получатель, 0)` для пачки. Принимает: окружение, вид. Ошибки: причина (нет dex.factory,
/// нет кошелька / секреты закрыты).
pub fn calls(ctx: &Ctx, f: &Family) -> Result<Vec<(Address, Bytes)>, Miss> {
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    Ok(recipients(ctx)?
        .into_iter()
        .map(|addr| {
            let c = ISpeFactory::swapFeeCall {
                addr,
                originalFee: U24::ZERO,
            };
            (factory, Bytes::from(c.abi_encode()))
        })
        .collect())
}

/// Разбор ответов swapFee(адрес, 0): все нули — Ok. Принимает: имя DEX, ответы. Ошибки: причина (своя комиссия у
/// кошелька/исполнителя или ответ не по ABI).
pub fn check(title: &str, answers: &[Bytes]) -> Result<(), Miss> {
    for b in answers {
        let fee = call::decode::<ISpeFactory::swapFeeCall>(b)?;
        if fee != U24::ZERO {
            return Err(own_fee(title, fee));
        }
    }
    Ok(())
}

/// То же по одному на блоке (load_v2v3.rs). Ошибки: как у calls/check и узел.
pub async fn verify(ctx: &Ctx, f: &Family, tag: &str) -> GatewayResult<()> {
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    for addr in recipients(ctx)? {
        let c = ISpeFactory::swapFeeCall {
            addr,
            originalFee: U24::ZERO,
        };
        let fee = call::view(ctx, factory, &c, tag).await?;
        if fee != U24::ZERO {
            return Err(own_fee(f.title, fee));
        }
    }
    Ok(())
}

/// Причина «у получателя своя комиссия». Ошибок нет.
fn own_fee(title: &str, fee: U24) -> Miss {
    miss(format!(
        "{title}: фабрика задала свою комиссию получателю (swapFee = {fee} из 1e6) — не равна fee() пула, \
         пул не торгуется (решение 274)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::keccak256;

    /// Селектор swapFee(address,uint24) = keccak256 сигнатуры (0xd35fc6af — он же в байткоде фабрики и пула).
    #[test]
    fn selector_matches() {
        assert_eq!(
            ISpeFactory::swapFeeCall::SELECTOR.to_vec(),
            keccak256(b"swapFee(address,uint24)")[..4].to_vec()
        );
        assert_eq!(ISpeFactory::swapFeeCall::SELECTOR, [0xd3, 0x5f, 0xc6, 0xaf]);
    }
}
