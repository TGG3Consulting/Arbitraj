//! Профили трёх V3-фабрик Base. Их собственные роутеры имеют разные callback, а обычный V5 их не принимает.
use super::{super::ctx::Ctx, families::Family};
use alloy_primitives::{Address, address};

pub const THRONE_QUOTER: Address = address!("f2e99648bf3b788690b6091b7ae19b507220c80b");

fn quoter(factory: Address) -> Option<Address> {
    const PROFILES: [(Address, Address); 3] = [
        (
            address!("7b72c4002ea7c276dd717b96b20f4956c5c904e7"),
            address!("f26600e17728f41adfb73d986e3deaf6df29f1c4"),
        ),
        (
            address!("e8839bf8175812691c6578c0fc80e721bc3e00fb"),
            THRONE_QUOTER,
        ),
        (
            address!("d6ab0566e7e60b67c50ac73ddff4e3ddcb829ec2"),
            address!("134d3c90a4a4c712cf0b548fe02003c9d934baba"),
        ),
    ];
    PROFILES
        .iter()
        .find(|(f, _)| *f == factory)
        .map(|(_, q)| *q)
}

pub fn valid(ctx: &Ctx, family: &Family) -> bool {
    ctx.chain_id() == Some(8453)
        && !ctx
            .dex_flag("dex.quoter_virtual", family.id)
            .unwrap_or(false)
        && ctx
            .dex_addr("dex.factory", family.id)
            .ok()
            .and_then(quoter)
            .is_some_and(|expected| ctx.dex_addr("dex.quoter", family.id).ok() == Some(expected))
}
