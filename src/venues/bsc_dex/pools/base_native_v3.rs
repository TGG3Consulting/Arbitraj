//! Файл: venues/bsc_dex/pools/base_native_v3.rs.
//! Что делает: проверяет, что официальный квотер и выбранный пул относятся к одной фабрике.
//! Кто вызывает: классификация, котировка, проба налогов и прямой модуль кошелька.
//! Настройки: dex.factory, dex.quoter, dex.quoter_virtual, dex.v3_quoter_method, dex.multicall3.
use super::super::{
    ctx::Ctx,
    node::{abi_dex::IQuoterV2, multicall},
};
use super::{abi_check::IPoolImmutables, abi_v2v3::IV3Factory, families::Family, registry::Entry};
use crate::{
    core::types::PoolRef,
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256, aliases::U24};

pub fn mixed_quoter(ctx: &Ctx, f: &Family) -> GatewayResult<(Address, bool)> {
    let address = ctx.dex_addr("dex.quoter", f.id)?;
    if address.is_zero() || ctx.dex_addr("dex.factory", f.id)?.is_zero() {
        return Err(ctx.err(
            ErrorCode::InvalidAddress,
            "Нулевой адрес фабрики или квотера V3",
        ));
    }
    match ctx.dex_text("dex.v3_quoter_method", f.id)?.as_str() {
        "quoteExactInputSingle" => Ok((address, false)),
        "quoteExactInputSingleV3" => Ok((address, true)),
        other => Err(ctx.err(
            ErrorCode::NotApplicable,
            format!("Неподдерживаемый метод V3-квотера: {other}"),
        )),
    }
}

pub fn valid(ctx: &Ctx, f: &Family) -> bool {
    !ctx.dex_flag("dex.quoter_virtual", f.id).unwrap_or(false) && mixed_quoter(ctx, f).is_ok()
}

pub fn request(
    ctx: &Ctx,
    e: &Entry,
    quoter: Address,
    fee: U24,
    data: Bytes,
) -> GatewayResult<(Address, Bytes)> {
    let registry = IV3Factory::getPoolCall {
        tokenA: e.pool.token0,
        tokenB: e.pool.token1,
        fee,
    };
    bound(ctx, e, quoter, registry.abi_encode().into(), data)
}

/// Проверяет принадлежность котировки тому же пулу без второго запроса узла.
pub(in crate::venues::bsc_dex) fn bound(
    ctx: &Ctx,
    e: &Entry,
    quoter: Address,
    registry: Bytes,
    data: Bytes,
) -> GatewayResult<(Address, Bytes)> {
    let PoolRef::Contract(pool) = e.pool.reference else {
        return Err(ctx.err(ErrorCode::InvalidAddress, "Для квотера нужен контракт пула"));
    };
    let factory = ctx.dex_addr("dex.factory", e.family.id)?;
    let read_factory = Bytes::from(IPoolImmutables::factoryCall {}.abi_encode());
    let calls = [
        (quoter, read_factory.clone()),
        (pool, read_factory),
        (factory, registry),
        (quoter, data),
    ];
    Ok((ctx.addr("dex.multicall3")?, multicall::encode(&calls)))
}

pub(in crate::venues::bsc_dex) fn checked_data(pool: Address, out: &[u8]) -> Result<Bytes, String> {
    let values = multicall::decode(out, 4).map_err(|e| e.message)?;
    let [q_factory, p_factory, registered, quote]: [Result<Bytes, String>; 4] =
        values
            .try_into()
            .map_err(|_| "Неполный ответ фабрики и квотера")?;
    let address = |r: Result<Bytes, String>| -> Result<Address, String> {
        IPoolImmutables::factoryCall::abi_decode_returns(&r?)
            .map_err(|_| "Ответ об адресе фабрики или пула не по ABI".to_owned())
    };
    let factory = address(q_factory)?;
    if factory.is_zero() || address(p_factory)? != factory || address(registered)? != pool {
        return Err(
            "Квотер и пул относятся к разным фабрикам или реестр вернул другой адрес".into(),
        );
    }
    quote
}

pub fn checked(pool: Address, out: &[u8]) -> Result<U256, String> {
    IQuoterV2::quoteExactInputSingleCall::abi_decode_returns(&checked_data(pool, out)?)
        .map(|result| result.amountOut)
        .map_err(|_| "Ответ квотера V3 не по ABI".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolValue;

    fn frame(
        quote_factory: Address,
        pool_factory: Address,
        registered: Address,
        success: bool,
    ) -> Vec<u8> {
        use multicall::IMulticall3::{Result as Item, aggregate3Call};
        let mut values = vec![];
        for address in [quote_factory, pool_factory, registered] {
            values.push(Item {
                success: true,
                returnData: address.abi_encode().into(),
            });
        }
        let amount = U256::from(77);
        let data = (amount, U256::from(1), U256::ZERO, U256::from(5)).abi_encode();
        values.push(Item {
            success,
            returnData: data.into(),
        });
        aggregate3Call::abi_encode_returns(&values)
    }

    #[test]
    fn only_the_registered_market_and_successful_quote_can_supply_the_amount() {
        let f = Address::repeat_byte(4);
        let other = Address::repeat_byte(7);
        let pool = Address::repeat_byte(2);
        assert_eq!(checked(pool, &frame(f, f, pool, true)), Ok(U256::from(77)));
        assert_eq!(
            checked(pool, &frame(other, other, pool, true)),
            Ok(U256::from(77))
        );
        assert!(checked(pool, &frame(f, other, pool, true)).is_err());
        assert!(checked(pool, &frame(f, f, other, true)).is_err());
        assert!(checked(pool, &frame(f, f, pool, false)).is_err());
    }
}
