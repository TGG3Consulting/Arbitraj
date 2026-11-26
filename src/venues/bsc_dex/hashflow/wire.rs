//! Проверка подписанной структуры и calldata по официальному ABI Hashflow.
//! https://github.com/hashflownetwork/x-protocol/blob/main/evm/abi/IHashflowRouter.json
use super::model::Request;
use alloy::sol;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::Deserialize;

sol! {
    struct RFQTQuote {
        address pool; address externalAccount; address trader; address effectiveTrader;
        address baseToken; address quoteToken;
        uint256 effectiveBaseTokenAmount; uint256 baseTokenAmount; uint256 quoteTokenAmount;
        uint256 quoteExpiry; uint256 nonce; bytes32 txid; bytes signature;
    }
    function tradeRFQT(RFQTQuote quote) external payable;
    event Trade(address trader, address effectiveTrader, bytes32 txid, address baseToken,
                address quoteToken, uint256 baseTokenAmount, uint256 quoteTokenAmount);
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Chain {
    pub chain_type: String,
    pub chain_id: u64,
}

impl Chain {
    pub fn is(&self, id: u64) -> bool {
        self.chain_type == "evm" && self.chain_id == id
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Data {
    base_chain: Chain,
    quote_chain: Chain,
    base_token: Address,
    quote_token: Address,
    pub base_token_amount: String,
    quote_token_amount: String,
    trader: Address,
    effective_trader: Option<Address>,
    external_account: Option<Address>,
    pub pool: Address,
    pub txid: B256,
    pub quote_expiry: u64,
    nonce: u64,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Firm {
    pub quote_data: Data,
    pub target_contract: Address,
    pub calldata: Bytes,
    signature: Bytes,
    value: Option<String>,
}

/// Целое в минимальных единицах. Десятичный ввод защищён от неоднозначных префиксов и округления.
pub(super) fn raw(s: &str) -> Result<U256, String> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Ожидается целое число минимальных единиц".into());
    }
    U256::from_str_radix(s, 10).map_err(|_| "Сумма превышает uint256".into())
}

/// Допуск только прямого вызова идентифицированному роутеру. Сервер иногда подписывает больший максимальный объём.
pub(super) fn verify(
    f: &Firm,
    chain: u64,
    router: Address,
    req: &Request,
    amount: U256,
    now: u64,
) -> Result<(U256, U256), String> {
    let d = &f.quote_data;
    if f.target_contract != router
        || !d.base_chain.is(chain)
        || !d.quote_chain.is(chain)
        || d.base_token != req.input
        || d.quote_token != req.output
        || d.trader != req.trader
        || d.effective_trader.unwrap_or(req.trader) != req.trader
        || d.quote_expiry <= now
        || d.pool.is_zero()
    {
        return Err("Сеть, адреса, кошелёк или срок подписанной RFQ расходятся с запросом".into());
    }
    let c =
        tradeRFQTCall::abi_decode_validate(&f.calldata).map_err(|_| "Неверный ABI tradeRFQT")?;
    if c.abi_encode().as_slice() != f.calldata.as_ref() {
        return Err("Неканоническая calldata Hashflow".into());
    }
    let q = c.quote;
    let max_in = raw(&d.base_token_amount)?;
    let max_out = raw(&d.quote_token_amount)?;
    // При отсутствии effectiveTrader контракт разрешает только прямого получателя согласно ответу API.
    if q.pool != d.pool
        || q.trader != req.trader
        || q.effectiveTrader != req.trader
        || q.externalAccount != d.external_account.unwrap_or_default()
        || q.baseToken != req.input
        || q.quoteToken != req.output
        || q.effectiveBaseTokenAmount != amount
        || amount > max_in
        || q.baseTokenAmount != max_in
        || q.quoteTokenAmount != max_out
        || q.txid != d.txid
        || q.quoteExpiry != U256::from(d.quote_expiry)
        || q.nonce != U256::from(d.nonce)
        || q.signature.is_empty()
        || q.signature != f.signature
        || amount.is_zero()
    {
        return Err("Поля calldata не совпадают с заявкой трейдера и подписанной RFQ".into());
    }
    let expected = max_out
        .checked_mul(amount)
        .and_then(|x| x.checked_div(max_in))
        .filter(|x| !x.is_zero())
        .ok_or("Выход RFQ равен нулю или превышает uint256")?;
    let value = match f.value.as_deref() {
        Some(s) if s.starts_with("0x") => {
            U256::from_str_radix(&s[2..], 16).map_err(|_| "Неверное value RFQ")?
        }
        Some(s) => raw(s)?,
        None if !req.input.is_zero() => U256::ZERO,
        None => return Err("Нет value нативной RFQ".into()),
    };
    if value
        != if req.input.is_zero() {
            amount
        } else {
            U256::ZERO
        }
    {
        return Err("value RFQ не соответствует входу".into());
    }
    Ok((expected, value))
}
