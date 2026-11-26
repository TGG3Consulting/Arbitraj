//! Ethereum Ring Swap. Канонические underlying и Few wrappers приняты по FewFactory,
//! обратным token()/factory(), decimals и CREATE2 Ring factory на блоке 26494988.
//! Порядок резервов совпадает с физическими fw-адресами, а торгуются underlying.
//! Контракты и ABI: https://ringprotocol.gitbook.io/ring/docs/deployments.
//! Изолированное воспроизведение: scripts/record_ring_ethereum.py.

use alloy::sol_types::SolValue;
use alloy_primitives::{Address, Bytes, U256, address, b256};
use rust_decimal::Decimal;

use super::super::ctx::Ctx;
use super::families::Family;

const FACTORY: Address = address!("eb2a625b704d73e82946d8d026e1f588eed06416");
const QUOTER: Address = address!("39d1d8fcc5e6eeaf567bce4e29b94fec956d3519");
const ROUTER: Address = address!("eff87a51f5abd015f1afcd5737bbab450ea15a24");
const PERMIT: Address = address!("000000000022d473030f116ddee9f6b43ac78ba3");
const INIT: alloy_primitives::B256 =
    b256!("a7ae6a5ec37f0c21bbdac560794258c4089b8ae3ffa6e3909b53c6091764a676");
const ASSETS: [(Address, Address); 5] = [
    (
        address!("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"),
        address!("a250cc729bb3323e7933022a67b52200fe354767"),
    ),
    (
        address!("dac17f958d2ee523a2206206994597c13d831ec7"),
        address!("ef87f4608e601e8564800265aee1c1ffadf73283"),
    ),
    (
        address!("a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"),
        address!("0492560fa7cfd6a85e50d8be3f77318994f8f429"),
    ),
    (
        address!("2260fac5e5542a773aa44fbcfedf7c193bc2c599"),
        address!("2078f336fdd260f708bec4a20c82b063274e1b23"),
    ),
    (
        address!("1f9840a85d5af5bf1d1762f925bdaddc4201f984"),
        address!("e8e1f50392bd61d0f8f48e8e7af51d3b8a52090a"),
    ),
];

/// Котировка и разрешение Permit2 должны принадлежать одной и той же принятой сети и фабрике.
pub fn profile(ctx: &Ctx, f: &Family) -> bool {
    ctx.chain_id() == Some(1)
        && [
            ("dex.factory", FACTORY),
            ("dex.router", QUOTER),
            ("dex.universal_router", ROUTER),
            ("dex.permit2", PERMIT),
        ]
        .iter()
        .all(|(name, addr)| ctx.dex_addr(name, f.id).ok() == Some(*addr))
        && ctx
            .dex_text("dex.universal_router_version", f.id)
            .ok()
            .as_deref()
            == Some("ring_few")
        && ctx.dex_text("dex.v2_fee_source", f.id).ok().as_deref() == Some("fixed")
        && ctx.dex_dec("dex.v2_fee_pct", f.id).ok() == Some(Decimal::new(3, 1))
}

fn wrapped(asset: Address) -> Option<Address> {
    ASSETS.iter().find(|(a, _)| *a == asset).map(|(_, w)| *w)
}

/// Принимаются только пары с WETH, для которых выполнена архивная проверка.
pub fn path(a: Address, b: Address) -> Option<[Address; 2]> {
    if a == b || (a != ASSETS[0].0 && b != ASSETS[0].0) {
        return None;
    }
    Some([wrapped(a)?, wrapped(b)?])
}

pub fn underlying(fw: Address) -> Option<Address> {
    ASSETS.iter().find(|(_, w)| *w == fw).map(|(a, _)| *a)
}

pub fn check_pool(pool: Address, fw0: Address, fw1: Address) -> bool {
    super::verify::v2_create2(FACTORY, fw0, fw1, INIT) == pool
}

/// FEW_V2_SWAP_EXACT_IN возвращает underlying получателю через unwrapTo.
/// Последующую проверку реального баланса добавляет штатный Plan::data.
pub fn exact_input(
    token_in: Address,
    token_out: Address,
    recipient: Address,
    amount: U256,
    minimum: U256,
    payer_user: bool,
) -> Option<(u8, Bytes)> {
    let path = path(token_in, token_out)?.to_vec();
    Some((
        0x23,
        Bytes::from((recipient, amount, minimum, path, payer_user, true).abi_encode_params()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_reserves_keep_their_physical_order_and_approval_uses_underlying() {
        let [
            (eth, w_eth),
            (usdt, w_usdt),
            (usdc, w_usdc),
            (wbtc, _),
            (uni, _),
        ] = ASSETS;
        let usdc_pool = address!("54222f404dcfac705322045f01d100380b871450");
        assert!(check_pool(usdc_pool, w_usdc, w_eth));
        assert_eq!(underlying(w_usdc), Some(usdc));
        assert_eq!(underlying(w_eth), Some(eth));
        assert_eq!(path(eth, usdt), Some([w_eth, w_usdt]));
        assert_eq!(path(w_eth, usdt), None);
        assert_eq!(path(wbtc, uni), None);
        let (command, data) =
            exact_input(usdt, eth, eth, U256::from(7), U256::from(4), true).unwrap();
        let args =
            <(Address, U256, U256, Vec<Address>, bool, bool)>::abi_decode_params(&data).unwrap();
        assert_eq!(command, 0x23);
        assert_eq!(
            args,
            (
                eth,
                U256::from(7),
                U256::from(4),
                vec![w_usdt, w_eth],
                true,
                true
            )
        );
        assert_eq!(super::super::exec_kind::of("ring_few_v2"), None);
    }
}
