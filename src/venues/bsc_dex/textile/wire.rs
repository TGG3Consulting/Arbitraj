//! Обычный подписанный LimitOrder Textile. Сверяет каждую выплату, срок, сумму комиссии
//! и исходные байты одиночного или группового поручения официальному Reactor.
use alloy::sol_types::{SolCall, SolStruct, SolValue};
use alloy_primitives::{Address, B256, Bytes, U256};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

alloy::sol! {
    struct OrderInfo {address reactor; address swapper; uint256 nonce; uint256 deadline; address additionalValidationContract; bytes additionalValidationData;}
    struct InputToken {address token; uint256 amount; uint256 maxAmount;}
    struct OutputToken {address token; uint256 amount; address recipient;}
    struct RawOrder {OrderInfo info; InputToken input; OutputToken[] outputs;}
    struct LimitOrder {OrderInfo info; address inputToken; uint256 inputAmount; OutputToken[] outputs;}
    struct SignedOrder {bytes order; bytes sig;}
    struct Terms {address sell; address buy; uint256 takerPays; uint256 minimum; uint256 deadline;}
    interface IReactor {
        function execute(SignedOrder order) external;
        function executeBatch(SignedOrder[] orders) external;
        function feeController() external view returns(address);
        function permit2() external view returns(address);
        event Fill(bytes32 indexed orderHash,address indexed filler,address indexed swapper,uint256 nonce);
    }
    interface IFee {function FEE_BPS() external view returns(uint256); function feeRecipient() external view returns(address);}
    interface IWallet {
        function settle(Terms terms,bytes action) external returns(uint256);
        function owner() external view returns(address);
        function reactor() external view returns(address);
        function reactorCodeHash() external view returns(bytes32);
        event Settled(address indexed payer,bytes32 indexed action,address indexed sellToken,address buyToken,uint256 spent,uint256 received);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Slice {
    pub hash: B256,
    pub maker: Address,
    pub nonce: U256,
    pub fee: U256,
    pub pays: U256,
    pub gets: U256,
}
#[derive(Clone, Copy)]
pub(super) struct Demand {
    pub chain: u64,
    pub reactor: Address,
    pub taker: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub exact_out: bool,
    pub bps: U256,
}
pub(super) struct Firm {
    pub action: Bytes,
    pub slices: Vec<Slice>,
    pub debit: U256,
    pub credit: U256,
    pub fee: U256,
    pub expiry: u64,
}
pub(super) fn uint(v: &Value) -> Option<U256> {
    v.as_str()?.parse().ok()
}
pub(super) fn addr(v: &Value) -> Option<Address> {
    v.as_str()?.parse().ok()
}
fn bytes(v: &Value) -> Option<Bytes> {
    v.as_str()?.parse().ok()
}
fn time(v: &Value) -> Option<u64> {
    DateTime::parse_from_rfc3339(v.as_str()?)
        .ok()?
        .timestamp()
        .try_into()
        .ok()
}

pub(super) fn amounts(v: &Value, qty: U256, exact_out: bool) -> Option<(U256, U256, U256)> {
    let debit = uint(&v["takerPays"])?;
    let credit = uint(&v["buyAmount"])?;
    let fee = uint(&v["feeAmount"])?;
    (uint(&v["sellAmount"])? == debit
        && !credit.is_zero()
        && fee < debit
        && (if exact_out { credit } else { debit }) == qty)
        .then_some((debit, credit, fee))
}

fn orders(raw: &Bytes) -> Option<Vec<SignedOrder>> {
    if raw.starts_with(&IReactor::executeCall::SELECTOR) {
        let x = IReactor::executeCall::abi_decode_validate(raw).ok()?;
        (x.abi_encode() == raw.as_ref()).then_some(vec![x.order])
    } else {
        let x = IReactor::executeBatchCall::abi_decode_validate(raw).ok()?;
        (!x.orders.is_empty() && x.abi_encode() == raw.as_ref()).then_some(x.orders)
    }
}

fn slice(d: &Demand, signed: &SignedOrder, v: &Value) -> Option<(Slice, u64)> {
    if bytes(&v["encodedOrder"])? != signed.order
        || bytes(&v["signature"])? != signed.sig
        || signed.sig.is_empty()
    {
        return None;
    }
    let r = RawOrder::abi_decode_validate(&signed.order).ok()?;
    let maker = r.info.swapper;
    if r.abi_encode() != signed.order.as_ref()
        || maker != addr(&v["maker"])?
        || maker.is_zero()
        || maker == d.taker
        || r.info.reactor != d.reactor
        || r.input.token != d.output
        || r.input.amount.is_zero()
        || r.input.amount != r.input.maxAmount
        || r.outputs.is_empty()
    {
        return None;
    }
    let pays = r.outputs.iter().try_fold(U256::ZERO, |sum, o| {
        if o.token != d.input || o.recipient != maker || o.amount.is_zero() {
            return None;
        }
        sum.checked_add(o.amount)
    })?;
    let fee = pays.checked_mul(d.bps)? / U256::from(10_000);
    let total = pays.checked_add(fee)?;
    // Суммы по срезам могут быть представлены до или с native fee. Совокупный debit и fee сверяются ниже.
    let sell = uint(&v["sellAmount"])?;
    if (sell != total && sell != pays)
        || uint(&v["feeAmount"])? != fee
        || uint(&v["buyAmount"])? != r.input.amount
        || fee.is_zero()
    {
        return None;
    }
    let deadline = r.info.deadline.try_into().ok()?;
    let nonce = r.info.nonce;
    let gets = r.input.amount;
    let hash = LimitOrder {
        info: r.info,
        inputToken: r.input.token,
        inputAmount: gets,
        outputs: r.outputs,
    }
    .eip712_hash_struct();
    Some((
        Slice {
            hash,
            maker,
            nonce,
            fee,
            pays,
            gets,
        },
        deadline,
    ))
}

pub(super) fn immediate(v: &Value, d: &Demand, now: u64) -> Option<Firm> {
    let data = &v["data"];
    let q = &data["quote"];
    let tx = &data["transactions"]["swap"];
    if data["status"] != "quoted"
        || !data["route"].is_null()
        || !data["attributionSuffix"].is_null()
        || addr(&q["reactor"])? != d.reactor
        || addr(&q["spender"])? != d.reactor
        || addr(&q["taker"])? != d.taker
        || tx["chainId"].as_u64() != Some(d.chain)
        || addr(&tx["to"])? != d.reactor
        || uint(&tx["value"])? != U256::ZERO
        || d.input == d.output
        || d.bps.is_zero()
        || d.bps > U256::from(5)
    {
        return None;
    }
    let (debit, credit, fee) = amounts(q, d.amount, d.exact_out)?;
    let action = bytes(&tx["data"])?;
    let raw = orders(&action)?;
    let list = q["orders"].as_array()?;
    if raw.len() != list.len() {
        return None;
    }
    let mut seen = HashSet::new();
    let mut slices = Vec::with_capacity(raw.len());
    let mut deadlines = Vec::with_capacity(raw.len());
    let (mut spend, mut buy, mut fees) = (U256::ZERO, U256::ZERO, U256::ZERO);
    for (signed, row) in raw.iter().zip(list) {
        let (s, deadline) = slice(d, signed, row)?;
        if !seen.insert((s.maker, s.nonce)) {
            return None;
        }
        spend = spend.checked_add(s.pays)?.checked_add(s.fee)?;
        buy = buy.checked_add(s.gets)?;
        fees = fees.checked_add(s.fee)?;
        slices.push(s);
        deadlines.push(deadline);
    }
    if (spend, buy, fees) != (debit, credit, fee)
        || *deadlines.iter().min()? != time(&q["orderDeadline"])?
        || q.get("latestOrderDeadline")
            .is_some_and(|v| time(v) != deadlines.iter().max().copied())
    {
        return None;
    }
    let expiry = time(&q["expiresAt"])?.min(*deadlines.iter().min()?);
    (expiry > now).then_some(Firm {
        action,
        slices,
        debit,
        credit,
        fee,
        expiry,
    })
}
