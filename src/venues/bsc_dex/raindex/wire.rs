//! Опубликованные ABI пяти поколений Raindex. Массив дополнительных подписей и callback data пусты.
//! Источники https://github.com/rainlanguage/raindex и реестр deployments/raindex.json.
use super::amount::Quantity;
use alloy::sol_types::{SolCall, SolEvent, SolValue};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
alloy::sol! {
    struct Eval3{address interpreter;address store;address expression;}
    struct Eval{address interpreter;address store;bytes bytecode;}
    struct IO4{address token;uint8 decimals;uint256 vaultId;}
    struct IO6{address token;bytes32 vaultId;}
    struct Order3{address owner;bool handleIO;Eval3 evaluable;IO4[] validInputs;IO4[] validOutputs;}
    struct Order4{address owner;Eval evaluable;IO4[] validInputs;IO4[] validOutputs;bytes32 nonce;}
    struct Order6{address owner;Eval evaluable;IO6[] validInputs;IO6[] validOutputs;bytes32 nonce;}
    struct Signed4{address signer;uint256[] context;bytes signature;}
    struct Signed6{address signer;bytes32[] context;bytes signature;}
    struct Signed7{address signer;bytes32 tag;bytes32[] context;bytes signature;}
    struct Leg3{Order3 order;uint256 inputIOIndex;uint256 outputIOIndex;Signed4[] signedContext;}
    struct Leg7{Order6 order;uint256 inputIOIndex;uint256 outputIOIndex;Signed7[] signedContext;}
    struct Leg4{Order4 order;uint256 inputIOIndex;uint256 outputIOIndex;Signed4[] signedContext;}
    struct Leg6{Order6 order;uint256 inputIOIndex;uint256 outputIOIndex;Signed6[] signedContext;}
    struct Take3{uint256 minimumInput;uint256 maximumInput;uint256 maximumIORatio;Leg3[] orders;bytes data;}
    struct Take5{bytes32 minimumInput;bytes32 maximumInput;bytes32 maximumIORatio;Leg6[] orders;bytes data;}
    struct Take7{bytes32 minimumIO;bytes32 maximumIO;bytes32 maximumIORatio;bool IOIsInput;Leg7[] orders;bytes data;}
    struct Take4{uint256 minimumInput;uint256 maximumInput;uint256 maximumIORatio;Leg4[] orders;bytes data;}
    struct Take6{bytes32 minimumIO;bytes32 maximumIO;bytes32 maximumIORatio;bool IOIsInput;Leg6[] orders;bytes data;}
    function orderExists(bytes32 hash) external view returns(bool);
    function quote3(Leg7 config) external view returns(bool live,bytes32 outputMax,bytes32 ioRatio);
    function takeOrders(Take3 config) external returns(uint256,uint256);
    function takeOrders3(Take5 config) external returns(bytes32,bytes32);
    function takeOrders5(Take7 config) external returns(bytes32,bytes32);
    event AddOrder(address sender,address expressionDeployer,Order3 order,bytes32 orderHash);
    event RemoveOrder(address sender,Order3 order,bytes32 orderHash);
    event TakeOrder(address sender,Leg3 config,uint256 input,uint256 output);
    event TakeOrderV4(address sender,Leg7 config,bytes32 input,bytes32 output);
    function quote(Leg4 config) external view returns(bool live,uint256 outputMax,uint256 ioRatio);
    function quote2(Leg6 config) external view returns(bool live,bytes32 outputMax,bytes32 ioRatio);
    function takeOrders2(Take4 config) external returns(uint256,uint256);
    function takeOrders4(Take6 config) external returns(bytes32,bytes32);
    event AddOrderV2(address sender,bytes32 orderHash,Order4 order);
    event RemoveOrderV2(address sender,bytes32 orderHash,Order4 order);
    event AddOrderV3(address sender,bytes32 orderHash,Order6 order);
    event RemoveOrderV3(address sender,bytes32 orderHash,Order6 order);
    event TakeOrderV2(address sender,Leg4 config,uint256 input,uint256 output);
    event TakeOrderV3(address sender,Leg6 config,bytes32 input,bytes32 output);
    struct Limits{address input;address output;uint256 minimumDebit;uint256 maximumDebit;uint256 minimumCredit;uint256 deadline;}
    function fill(Limits l,bytes action) external returns(uint256 spent,uint256 received);
    event Settled(address indexed payer,bytes32 indexed orderCall,address indexed input,address output,uint256 spent,uint256 received);
}
#[derive(Clone)]
pub(super) enum Order {
    V3(Order3),
    V4(Order4),
    Float(u8, Order6),
}
impl Order {
    pub(super) fn decode(version: u8, raw: &[u8], hash: B256) -> Option<Self> {
        if raw.len() > 65536 || keccak256(raw) != hash {
            return None;
        }
        match version {
            3 => {
                let o = Order3::abi_decode_validate(raw).ok()?;
                (o.abi_encode() == raw).then_some(Self::V3(o))
            }
            4 => {
                let o = Order4::abi_decode_validate(raw).ok()?;
                (o.abi_encode() == raw).then_some(Self::V4(o))
            }
            5..=7 => {
                let o = Order6::abi_decode_validate(raw).ok()?;
                (o.abi_encode() == raw).then_some(Self::Float(version, o))
            }
            _ => None,
        }
    }
    pub(super) fn added(version: u8, data: &[u8]) -> Option<(B256, Bytes)> {
        match version {
            3 => {
                let e = AddOrder::decode_raw_log([AddOrder::SIGNATURE_HASH], data).ok()?;
                Some((e.orderHash, e.order.abi_encode().into()))
            }
            4 => {
                let e = AddOrderV2::decode_raw_log([AddOrderV2::SIGNATURE_HASH], data).ok()?;
                Some((e.orderHash, e.order.abi_encode().into()))
            }
            5..=7 => {
                let e = AddOrderV3::decode_raw_log([AddOrderV3::SIGNATURE_HASH], data).ok()?;
                Some((e.orderHash, e.order.abi_encode().into()))
            }
            _ => None,
        }
    }
    pub(super) fn owner(&self) -> Address {
        match self {
            Self::V3(o) => o.owner,
            Self::V4(o) => o.owner,
            Self::Float(_, o) => o.owner,
        }
    }
    pub(super) fn pairs(&self) -> (Vec<Address>, Vec<Address>) {
        match self {
            Self::V3(o) => (
                o.validInputs.iter().map(|i| i.token).collect(),
                o.validOutputs.iter().map(|i| i.token).collect(),
            ),
            Self::V4(o) => (
                o.validInputs.iter().map(|i| i.token).collect(),
                o.validOutputs.iter().map(|i| i.token).collect(),
            ),
            Self::Float(_, o) => (
                o.validInputs.iter().map(|i| i.token).collect(),
                o.validOutputs.iter().map(|i| i.token).collect(),
            ),
        }
    }
    fn v3(o: &Order3, i: usize, j: usize) -> Leg3 {
        Leg3 {
            order: o.clone(),
            inputIOIndex: U256::from(i),
            outputIOIndex: U256::from(j),
            signedContext: vec![],
        }
    }
    fn v7(o: &Order6, i: usize, j: usize) -> Leg7 {
        Leg7 {
            order: o.clone(),
            inputIOIndex: U256::from(i),
            outputIOIndex: U256::from(j),
            signedContext: vec![],
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
    pub(super) fn quote(&self, i: usize, j: usize) -> Option<Bytes> {
        Some(match self {
            Self::V3(_) => return None,
            Self::Float(7, o) => quote3Call {
                config: Self::v7(o, i, j),
            }
            .abi_encode()
            .into(),
            Self::V4(o) => quoteCall {
                config: Self::v4(o, i, j),
            }
            .abi_encode()
            .into(),
            Self::Float(_, o) => quote2Call {
                config: Self::v6(o, i, j),
            }
            .abi_encode()
            .into(),
        })
    }
    pub(super) fn price(&self, response: &[u8]) -> Option<(Quantity, Quantity, B256)> {
        match self {
            Self::V3(_) => None,
            Self::V4(_) => {
                let r = quoteCall::abi_decode_returns_validate(response).ok()?;
                r.live.then_some((
                    Quantity::v4(r.outputMax)?,
                    Quantity::v4(r.ioRatio)?,
                    B256::from(r.ioRatio),
                ))
            }
            Self::Float(..) => {
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
            Self::V3(o) if fixed_receive => takeOrdersCall {
                config: Take3 {
                    minimumInput: qty,
                    maximumInput: qty,
                    maximumIORatio: U256::from_be_bytes(ratio.0),
                    orders: vec![Self::v3(o, i, j)],
                    data: Bytes::new(),
                },
            }
            .abi_encode()
            .into(),
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
            Self::Float(5, o) if fixed_receive => {
                let q = Quantity::units(qty, dec)?;
                takeOrders3Call {
                    config: Take5 {
                        minimumInput: q,
                        maximumInput: q,
                        maximumIORatio: ratio,
                        orders: vec![Self::v6(o, i, j)],
                        data: Bytes::new(),
                    },
                }
                .abi_encode()
                .into()
            }
            Self::Float(7, o) => {
                let q = Quantity::units(qty, dec)?;
                takeOrders5Call {
                    config: Take7 {
                        minimumIO: q,
                        maximumIO: q,
                        maximumIORatio: ratio,
                        IOIsInput: fixed_receive,
                        orders: vec![Self::v7(o, i, j)],
                        data: Bytes::new(),
                    },
                }
                .abi_encode()
                .into()
            }
            Self::Float(6, o) => {
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
