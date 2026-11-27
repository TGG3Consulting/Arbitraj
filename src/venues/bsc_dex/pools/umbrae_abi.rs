//! ABI проверенных реализаций Umbrae и события, требующие перечитывания.
use super::family_variant::Variant;
use alloy::sol;
use alloy_primitives::{B256, keccak256};
sol! {
    interface IMarket {
        event PairCreated(address indexed x,address indexed y,uint16 indexed step,address pair,uint256 index,uint24 active,uint16 factor);
        function tokenX() view returns (address);
        function tokenY() view returns (address);
        function binStep() view returns (uint16);
        function getPairBaseFactor(address pair) view returns(uint16);
        function getPair(address a,address b,uint16 step,uint16 factor) view returns(address);
        function getAllPairs(address a,address b) view returns(address[]);
        function beacon() view returns(address);
        function implementation() view returns(address);
        function FACTORY() view returns(address);
        function dlmm() view returns(address);
        function damm() view returns(address);
        function quoteSwapDetailed(address pair,bool forward,uint256 amount) view returns(uint256 out,uint256 used,uint24 bin,uint256 bins,uint256 fee,bool capped);
        function getAmountOut(uint256 amount,bool xToY) view returns(uint256);
    }
}
pub(crate) fn swap_topic(variant: Variant) -> B256 {
    keccak256(if variant == Variant::UmbraeDlmm {
        "Swap(address,address,uint256,uint256,uint24,bool)"
    } else {
        "Swap(address,address,uint256,uint256,uint256,uint256,uint16)"
    })
}
pub(super) fn topics(variant: Variant) -> Vec<B256> {
    let sigs: &[&str] = if variant == Variant::UmbraeDlmm {
        &[
            "ReservesUpdated(uint128,uint128,uint256)",
            "FeeParametersUpdated(uint16,uint16,uint16,uint16,uint16,uint24)",
            "VolatilityUpdated(uint128,uint256)",
            "Paused()",
            "Unpaused()",
            "LiquidityAdded(address,address,uint256[],uint256[],uint256[],uint256,uint256,uint24)",
            "LiquidityRemoved(address,address,uint256[],uint256[],uint256,uint256,uint256,uint256,uint24)",
        ]
    } else {
        &[
            "Sync(uint112,uint112)",
            "FeeParametersUpdated(uint16,uint16,uint16)",
            "FeeTokenUpdated(address)",
            "LPFeesClaimed(address,address,uint256)",
            "ProtocolFeesClaimed(address,uint256)",
        ]
    };
    std::iter::once(swap_topic(variant))
        .chain(sigs.iter().map(|s| keccak256(s.as_bytes())))
        .collect()
}
