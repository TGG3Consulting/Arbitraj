//! Поручение Tokenlon PMMV5: точная сумма fillOrKill, хеш 0x v2 и отдельная подпись получателя.
//! Источник: verified PMM 0x8d90113a1e286a5ab3e496fbd1853f265e5913c6, LibOrder, LibEncoder и _assertTransaction.
pub(super) mod abi;
mod hash;
mod signature;
use signature::{maker_signature, vrs};

use super::{receipt::Proof, roles::FrozenRoles};
use abi::{Order, fillCall, fillOrKillOrderCall, toPMMCall};
use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, B256, Bytes, Signature, U256};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(in crate::venues::bsc_dex) struct Terms {
    pub roles: FrozenRoles,
    pub payer: Address,
    pub recipient: Address,
    pub input: Address,
    pub output: Address,
    pub spent: U256,
    pub minimum: U256,
}
pub(in crate::venues::bsc_dex) struct Prepared {
    pub proof: Proof,
    pub sign_digest: B256,
    user_salt: U256,
    inner: Bytes,
}

pub(super) fn asset(token: Address) -> Bytes {
    [&[0xf4, 0x72, 0x61, 0xb0][..], &token.abi_encode()]
        .concat()
        .into()
}
fn quantity(v: &Value) -> Option<U256> {
    let n = v
        .as_str()
        .map(str::to_owned)
        .or_else(|| v.as_u64().map(|u| u.to_string()))?;
    (!n.is_empty() && n.bytes().all(|x| x.is_ascii_digit()))
        .then(|| U256::from_str_radix(&n, 10).ok())
        .flatten()
}
/// Заявка из lastOrder SDK. Приём подписи контрактного maker завершается проверкой всего исходного вызова.
pub(in crate::venues::bsc_dex) fn prepare(
    raw: &Value,
    t: &Terms,
    user_salt: U256,
    now: u64,
) -> Option<Prepared> {
    let addr = |k: &str| raw[k].as_str()?.parse::<Address>().ok();
    let num = |k: &str| quantity(&raw[k]);
    let bytes = |k: &str| raw[k].as_str()?.parse::<Bytes>().ok();
    if t.recipient != t.payer
        || raw["protocol"] != "PMMV5"
        || addr("exchangeAddress")? != t.roles.zero_exchange.address
        || addr("makerAssetAddress")? != t.output
        || addr("takerAssetAddress")? != t.input
    {
        return None;
    }
    let order = Order {
        makerAddress: addr("makerAddress")?,
        takerAddress: addr("takerAddress")?,
        feeRecipientAddress: addr("feeRecipientAddress")?,
        senderAddress: addr("senderAddress")?,
        makerAssetAmount: num("makerAssetAmount")?,
        takerAssetAmount: num("takerAssetAmount")?,
        makerFee: num("makerFee")?,
        takerFee: num("takerFee")?,
        expirationTimeSeconds: num("expirationTimeSeconds")?,
        salt: num("salt")?,
        makerAssetData: bytes("makerAssetData")?,
        takerAssetData: bytes("takerAssetData")?,
    };
    if num("feeFactor")? != order.salt & U256::from(u16::MAX) {
        return None;
    }
    let inner = fillOrKillOrderCall {
        order,
        takerAssetFillAmount: t.spent,
        signature: bytes("makerWalletSignature")?,
    };
    Prepared::new(inner, t, user_salt, now)
}

impl Prepared {
    fn new(inner: fillOrKillOrderCall, t: &Terms, user_salt: U256, now: u64) -> Option<Self> {
        let o = &inner.order;
        let domain = hash::domain(t.roles.zero_exchange.address);
        let order_hash = hash::order(domain, o);
        let pmm = t.roles.pmm.address;
        let fee = (o.salt & U256::from(u16::MAX)).to::<u16>();
        if t.payer.is_zero()
            || t.recipient.is_zero()
            || t.input.is_zero()
            || t.output.is_zero()
            || t.input == t.output
            || t.spent.is_zero()
            || t.minimum.is_zero()
            || o.makerAddress.is_zero()
            || [t.payer, pmm].contains(&o.makerAddress)
            || o.takerAddress != pmm
            || o.senderAddress != pmm
            || o.feeRecipientAddress != t.payer
            || o.takerAssetAmount != t.spent
            || inner.takerAssetFillAmount != t.spent
            || !o.makerFee.is_zero()
            || !o.takerFee.is_zero()
            || fee >= 10000
            || o.takerAssetData != asset(t.input)
            || o.makerAssetData != asset(t.output)
            || !maker_signature(&inner.signature, order_hash, o.makerAddress)
        {
            return None;
        }
        let expiry = u64::try_from(o.expirationTimeSeconds).ok()?;
        let net = o.makerAssetAmount.checked_mul(U256::from(10000 - fee))? / U256::from(10000);
        if expiry <= now || net < t.minimum {
            return None;
        }
        let data: Bytes = inner.abi_encode().into();
        let tx_hash = hash::transaction(domain, pmm, user_salt, &data);
        Some(Self {
            proof: Proof {
                terms: t.clone(),
                maker: o.makerAddress,
                order_hash,
                tx_hash,
                gross: o.makerAssetAmount,
                net,
                expiry,
                fee,
            },
            sign_digest: hash::taker(tx_hash, t.recipient),
            user_salt,
            inner: data,
        })
    }
    pub(in crate::venues::bsc_dex) fn calldata(&self, signature: &Signature) -> Option<Bytes> {
        if self.proof.terms.recipient != self.proof.terms.payer
            || signature
                .recover_address_from_prehash(&self.sign_digest)
                .ok()?
                != self.proof.terms.payer
        {
            return None;
        }
        let bytes = signature.as_bytes();
        let user_signature = [
            &[27 + u8::from(signature.v())][..],
            &bytes[..64],
            self.proof.terms.recipient.as_slice(),
            &[4],
        ]
        .concat()
        .into();
        let payload = fillCall {
            userSalt: self.user_salt,
            data: self.inner.clone(),
            userSignature: user_signature,
        }
        .abi_encode()
        .into();
        Some(toPMMCall { payload }.abi_encode().into())
    }
    /// Проверка исходного calldata при восстановлении. Текущий час не заменяет время блока включения.
    pub(in crate::venues::bsc_dex) fn from_calldata(
        data: &[u8],
        t: &Terms,
        block_time: u64,
    ) -> Option<Self> {
        let outer = toPMMCall::abi_decode_validate(data).ok()?;
        let f = fillCall::abi_decode_validate(&outer.payload).ok()?;
        let inner = fillOrKillOrderCall::abi_decode_validate(&f.data).ok()?;
        if f.abi_encode() != outer.payload.as_ref()
            || outer.abi_encode() != data
            || inner.abi_encode() != f.data.as_ref()
        {
            return None;
        }
        let p = Self::new(inner, t, f.userSalt, block_time)?;
        let s = &f.userSignature;
        if !(s.len() == 85 || s.len() == 86 && s[85] == 4)
            || s[65..85] != t.recipient[..]
            || vrs(s)?.recover_address_from_prehash(&p.sign_digest).ok()? != t.payer
        {
            return None;
        }
        Some(p)
    }
}
