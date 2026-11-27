//! Опубликованные ABI OrderBookV4 и OrderBookV6. Массив дополнительных подписей и callback data пусты.
//! Источники https://github.com/rainlanguage/raindex и реестр deployments/raindex.json.
use super::amount::Quantity;
use alloy::sol_types::{SolCall, SolEvent, SolValue};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
alloy::sol! {
    struct Eval{address interpreter;address store;bytes bytecode;}
    struct IO4{address token;uint8 decimals;uint256 vaultId;}
    struct IO6{address token;bytes32 vaultId;}
    struct Order4{address owner;Eval evaluable;IO4[] validInputs;IO4[] validOutputs;bytes32 nonce;}
    struct Order6{address owner;Eval evaluable;IO6[] validInputs;IO6[] validOutputs;bytes32 nonce;}
    struct Signed4{address signer;uint256[] context;bytes signature;}
    struct Signed6{address signer;bytes32[] context;bytes signature;}
    struct Leg4{Order4 order;uint256 inputIOIndex;uint256 outputIOIndex;Signed4[] signedContext;}
    struct Leg6{Order6 order;uint256 inputIOIndex;uint256 outputIOIndex;Signed6[] signedContext;}
    struct Take4{uint256 minimumInput;uint256 maximumInput;uint256 maximumIORatio;Leg4[] orders;bytes data;}
    struct Take6{bytes32 minimumIO;bytes32 maximumIO;bytes32 maximumIORatio;bool IOIsInput;Leg6[] orders;bytes data;}
    function quote(Leg4 config) external view returns(bool live,uint256 outputMax,uint256 ioRatio);
    function quote2(Leg6 config) external view returns(bool live,bytes32 outputMax,bytes32 ioRatio);
    function takeOrders2(Take4 config) external returns(uint256,uint256);
    function takeOrders4(Take6 config) external returns(bytes32,bytes32);
    event AddOrderV2(address sender,bytes32 orderHash,Order4 order);
    event AddOrderV3(address sender,bytes32 orderHash,Order6 order);
    event TakeOrderV2(address sender,Leg4 config,uint256 input,uint256 output);
    event TakeOrderV3(address sender,Leg6 config,bytes32 input,bytes32 output);
    struct Limits{address input;address output;uint256 minimumDebit;uint256 maximumDebit;uint256 minimumCredit;uint256 deadline;}
    function fill(Limits l,bytes action) external returns(uint256 spent,uint256 received);
    event Settled(address indexed payer,bytes32 indexed orderCall,address indexed input,address output,uint256 spent,uint256 received);
}
#[derive(Clone)]
pub(super) enum Order {
    V4(Order4),
    V6(Order6),
}
impl Order {
    pub(super) fn decode(version: u8, raw: &[u8], hash: B256) -> Option<Self> {
        if raw.len() > 65536 || keccak256(raw) != hash {
            return None;
        }
        match version {
            4 => {
                let o = Order4::abi_decode_validate(raw).ok()?;
                (o.abi_encode() == raw).then_some(Self::V4(o))
            }
            6 => {
                let o = Order6::abi_decode_validate(raw).ok()?;
                (o.abi_encode() == raw).then_some(Self::V6(o))
            }
            _ => None,
        }
    }
    pub(super) fn added(version: u8, data: &[u8]) -> Option<(B256, Bytes)> {
        match version {
            4 => {
                let e = AddOrderV2::decode_raw_log([AddOrderV2::SIGNATURE_HASH], data).ok()?;
                Some((e.orderHash, e.order.abi_encode().into()))
            }
            6 => {
                let e = AddOrderV3::decode_raw_log([AddOrderV3::SIGNATURE_HASH], data).ok()?;
                Some((e.orderHash, e.order.abi_encode().into()))
            }
            _ => None,
        }
    }
    pub(super) fn owner(&self) -> Address {
        match self {
            Self::V4(o) => o.owner,
            Self::V6(o) => o.owner,
        }
    }
    pub(super) fn pairs(&self) -> (Vec<Address>, Vec<Address>) {
        match self {
            Self::V4(o) => (
                o.validInputs.iter().map(|i| i.token).collect(),
                o.validOutputs.iter().map(|i| i.token).collect(),
            ),
            Self::V6(o) => (
                o.validInputs.iter().map(|i| i.token).collect(),
                o.validOutputs.iter().map(|i| i.token).collect(),
            ),
        }
    }
    fn v4(o: &Order4, i: usize, j: usize) -> Leg4 {
        Leg4 {
            order: o.clone(),
            inputIOIndex: U256::from(i),
            outputIOIndex: U256::from(j),
            signedContext: vec![],
        }
    }
    fn v6(o: &Order6, i: usize, j: usize) -> Leg6 {
        Leg6 {
            order: o.clone(),
            inputIOIndex: U256::from(i),
            outputIOIndex: U256::from(j),
            signedContext: vec![],
        }
    }
    pub(super) fn quote(&self, i: usize, j: usize) -> Bytes {
        match self {
            Self::V4(o) => quoteCall {
                config: Self::v4(o, i, j),
            }
            .abi_encode()
            .into(),
            Self::V6(o) => quote2Call {
                config: Self::v6(o, i, j),
            }
            .abi_encode()
            .into(),
        }
    }
    pub(super) fn price(&self, response: &[u8]) -> Option<(Quantity, Quantity, B256)> {
        match self {
            Self::V4(_) => {
                let r = quoteCall::abi_decode_returns_validate(response).ok()?;
                r.live.then_some((
                    Quantity::v4(r.outputMax)?,
                    Quantity::v4(r.ioRatio)?,
                    B256::from(r.ioRatio),
                ))
            }
            Self::V6(_) => {
                let r = quote2Call::abi_decode_returns_validate(response).ok()?;
                r.live.then_some((
                    Quantity::float(r.outputMax)?,
                    Quantity::float(r.ioRatio)?,
                    r.ioRatio,
                ))
            }
        }
    }
    pub(super) fn take(
        &self,
        i: usize,
        j: usize,
        qty: U256,
        dec: u8,
        fixed_receive: bool,
        ratio: B256,
    ) -> Option<Bytes> {
        Some(match self {
            Self::V4(o) if fixed_receive => takeOrders2Call {
                config: Take4 {
                    minimumInput: qty,
                    maximumInput: qty,
                    maximumIORatio: U256::from_be_bytes(ratio.0),
                    orders: vec![Self::v4(o, i, j)],
                    data: Bytes::new(),
                },
            }
            .abi_encode()
            .into(),
            Self::V6(o) => {
                let q = Quantity::units(qty, dec)?;
                takeOrders4Call {
                    config: Take6 {
                        minimumIO: q,
                        maximumIO: q,
                        maximumIORatio: ratio,
                        IOIsInput: fixed_receive,
                        orders: vec![Self::v6(o, i, j)],
                        data: Bytes::new(),
                    },
                }
                .abi_encode()
                .into()
            }
            _ => return None,
        })
    }
}
