//! See Dexalot/contracts@993df50c, MainnetRFQ.sol#L675 and DexalotRFQ.sol#L525.
use alloy::sol_types::{SolCall, SolStruct};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde_json::Value;
#[allow(clippy::too_many_arguments)]
mod abi {
    alloy::sol! {
        struct Order {uint256 nonceAndMeta;uint128 expiry;address makerAsset;address takerAsset;address maker;address taker;uint256 makerAmount;uint256 takerAmount;}
        interface IRfq {
            function VERSION() external view returns(bytes32);
            function getAllowedRFQs() external view returns(address[]);
            function completedSwaps(uint256 bucket) external view returns(uint256);
            function hasRole(bytes32 role,address grantee) external view returns(bool);
            function isValidSignature(bytes32 digest,bytes signature) external view returns(bytes4);
            function simpleSwap(Order order,bytes signature) external payable;
            function partialSwap(Order order,bytes signature,uint256 amount) external payable;
            event SwapExecuted(uint256 indexed nonceAndMeta,address taker,bytes32 destTrader,uint32 destChainId,address srcAsset,bytes32 destAsset,uint256 srcAmount,uint256 destAmount);
        }
        interface IWallet{
            function router() external view returns(address);
            function wrapped() external view returns(address);
            function owner() external view returns(address);
            function routerHash() external view returns(bytes32);
            function execute(Order order,bytes signature,uint256 maximum,uint256 minimum,bool partialFill,uint256 deadline) external returns(uint256 spent,uint256 received);
            event Filled(address indexed payer,address indexed venue,uint256 indexed nonce,address input,address output,uint256 spent,uint256 received);
        }
    }
}
pub(super) use abi::{IRfq, IWallet, Order};
fn integer(v: &Value) -> Option<U256> {
    if let Some(n) = v.as_u64() {
        return Some(U256::from(n));
    }
    let numeric;
    let s = if let Some(n) = v.as_number() {
        numeric = n.to_string();
        numeric.as_str()
    } else {
        v.as_str()?
    };
    if let Some(n) = s.strip_prefix("0x") {
        U256::from_str_radix(n, 16).ok()
    } else {
        U256::from_str_radix(s, 10).ok()
    }
}
impl Order {
    pub(super) fn parse(v: &Value) -> Option<Self> {
        let a = |k: &str| v.get(k)?.as_str()?.parse::<Address>().ok();
        let n = |k: &str| integer(v.get(k)?);
        let o = Self {
            nonceAndMeta: n("nonceAndMeta")?,
            expiry: n("expiry")?.try_into().ok()?,
            makerAsset: a("makerAsset")?,
            takerAsset: a("takerAsset")?,
            maker: a("maker")?,
            taker: a("taker")?,
            makerAmount: n("makerAmount")?,
            takerAmount: n("takerAmount")?,
        };
        (!o.maker.is_zero()
            && !o.taker.is_zero()
            && o.takerAsset != o.makerAsset
            && !o.takerAmount.is_zero()
            && !o.makerAmount.is_zero())
        .then_some(o)
    }
    pub(super) fn deadline(&self) -> u64 {
        u64::from(self.expiry as u32)
    }
    pub(super) fn payee(&self) -> Address {
        Address::from_slice(&self.nonceAndMeta.to_be_bytes::<32>()[..20])
    }
    pub(super) fn constant_output(&self) -> bool {
        self.nonceAndMeta.to_be_bytes::<32>()[20] <= 7
    }
    pub(super) fn digest(&self, chain: u64, version: &'static str) -> B256 {
        // Both the later immutable maker and the 1.2.8 proxy use their own verifying address.
        let domain = alloy::sol_types::eip712_domain! {name:"Dexalot",version:version,chain_id:chain,verifying_contract:self.maker,};
        self.eip712_signing_hash(&domain)
    }
    pub(super) fn data(
        &self,
        signature: Bytes,
        module: bool,
        maximum: U256,
        minimum: U256,
        partial: bool,
    ) -> Bytes {
        if module {
            IWallet::executeCall {
                order: self.clone(),
                signature,
                maximum,
                minimum,
                partialFill: partial,
                deadline: U256::from(self.deadline()),
            }
            .abi_encode()
            .into()
        } else if partial {
            IRfq::partialSwapCall {
                order: self.clone(),
                signature,
                amount: maximum.min(self.takerAmount),
            }
            .abi_encode()
            .into()
        } else {
            IRfq::simpleSwapCall {
                order: self.clone(),
                signature,
            }
            .abi_encode()
            .into()
        }
    }
}
