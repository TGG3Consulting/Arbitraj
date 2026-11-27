//! AirSwap's deployed V4/V5 SwapERC20 and the exact owner settlement envelope.
//! https://github.com/airswap/airswap-about/blob/6363ffe1c1efd06d35f11fe1832c5e504341faad/technology/protocols.md
use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[allow(clippy::too_many_arguments)]
mod abi {
    alloy::sol! {
        interface ISwap {
            function swap(address recipient,uint256 nonce,uint256 expiry,address signerWallet,address signerToken,uint256 signerAmount,address senderToken,uint256 senderAmount,uint8 v,bytes32 r,bytes32 s) external;
            function swapAnySender(address recipient,uint256 nonce,uint256 expiry,address signerWallet,address signerToken,uint256 signerAmount,address senderToken,uint256 senderAmount,uint8 v,bytes32 r,bytes32 s) external;
            function DOMAIN_SEPARATOR() external view returns(bytes32);
            function ORDER_TYPEHASH() external view returns(bytes32);
            function protocolFee() external view returns(uint256);
            function authorized(address signerWallet) external view returns(address);
            function nonceUsed(address signatory,uint256 nonce) external view returns(bool);
        }
        interface IV4 {
            event SwapERC20(uint256 indexed nonce,address indexed signerWallet,address signerToken,uint256 signerAmount,uint256 protocolFee,address indexed senderWallet,address senderToken,uint256 senderAmount);
        }
        interface IV5 {event SwapERC20(uint256 indexed nonce,address indexed signerWallet);}
        struct OrderTerms {uint256 nonce;uint256 expiry;address maker;address input;uint256 amount;address output;uint256 quoted;bool anySender;uint8 v;bytes32 r;bytes32 s;}
        interface IWallet {
            function owner() external view returns(address);
            function exchange() external view returns(address);
            function exchangeHash() external view returns(bytes32);
            function execute(OrderTerms q,uint256 minimum,uint256 deadline) external returns(uint256 spent,uint256 received);
            event Paid(address indexed payer,address indexed maker,uint256 indexed nonce,address input,address output,uint256 spent,uint256 received);
        }
    }
}
pub(super) use abi::{ISwap, IV4, IV5, IWallet};
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(super) enum Version {
    V4,
    V5,
}
#[derive(Clone, Debug)]
pub(super) struct Order {
    pub maker: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub quoted: U256,
    pub fee: U256,
    pub nonce: U256,
    pub expiry: u64,
    pub sender: Option<Address>,
    v: u8,
    r: B256,
    s: B256,
}
pub(super) const TYPE: &str = "OrderERC20(uint256 nonce,uint256 expiry,address signerWallet,address signerToken,uint256 signerAmount,uint256 protocolFee,address senderWallet,address senderToken,uint256 senderAmount)";
fn integer(v: &Value) -> Option<U256> {
    if let Some(n) = v.as_u64() {
        return Some(U256::from(n));
    }
    let n = v.as_str()?;
    if n.is_empty() || !n.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    U256::from_str_radix(n, 10).ok()
}
impl Order {
    pub(super) fn read(v: &Value) -> Option<Self> {
        let addr = |k: &str| v.get(k)?.as_str()?.parse::<Address>().ok();
        let qty = |k: &str| integer(v.get(k)?);
        let word = |k: &str| v.get(k)?.as_str()?.parse::<B256>().ok();
        let sender = match v.get("senderWallet") {
            Some(_) => Some(addr("senderWallet")?),
            None => None,
        };
        let order = Self {
            maker: addr("signerWallet")?,
            input: addr("senderToken")?,
            output: addr("signerToken")?,
            amount: qty("senderAmount")?,
            quoted: qty("signerAmount")?,
            fee: qty("protocolFee")?,
            nonce: qty("nonce")?,
            expiry: qty("expiry")?.try_into().ok()?,
            sender,
            v: qty("v")?.try_into().ok()?,
            r: word("r")?,
            s: word("s")?,
        };
        (!order.maker.is_zero()
            && !order.input.is_zero()
            && !order.output.is_zero()
            && order.input != order.output
            && !order.amount.is_zero()
            && !order.quoted.is_zero())
        .then_some(order)
    }
    pub(super) fn digest(&self, domain: B256, sender: Address) -> B256 {
        let terms = keccak256(
            (
                keccak256(TYPE.as_bytes()),
                self.nonce,
                U256::from(self.expiry),
                self.maker,
                self.output,
                self.quoted,
                self.fee,
                sender,
                self.input,
                self.amount,
            )
                .abi_encode(),
        );
        let mut b = Vec::from([0x19, 0x01]);
        b.extend_from_slice(domain.as_slice());
        b.extend_from_slice(terms.as_slice());
        keccak256(b)
    }
    pub(super) fn data(&self, payer: Address, protected: bool, any: bool, minimum: U256) -> Bytes {
        let expiry = U256::from(self.expiry);
        if protected {
            let q = abi::OrderTerms {
                nonce: self.nonce,
                expiry,
                maker: self.maker,
                input: self.input,
                amount: self.amount,
                output: self.output,
                quoted: self.quoted,
                anySender: any,
                v: self.v,
                r: self.r,
                s: self.s,
            };
            IWallet::executeCall {
                q,
                minimum,
                deadline: expiry,
            }
            .abi_encode()
            .into()
        } else {
            let mut data = ISwap::swapCall {
                recipient: payer,
                nonce: self.nonce,
                expiry,
                signerWallet: self.maker,
                signerToken: self.output,
                signerAmount: self.quoted,
                senderToken: self.input,
                senderAmount: self.amount,
                v: self.v,
                r: self.r,
                s: self.s,
            }
            .abi_encode();
            if any {
                data[..4].copy_from_slice(&ISwap::swapAnySenderCall::SELECTOR);
            }
            data.into()
        }
    }
}
