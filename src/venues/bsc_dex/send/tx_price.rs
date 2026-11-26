//! Файл: venues/bsc_dex/send/tx_price.rs
//! Что делает: решение 435 (задача 433-д) — цена газа транзакции по модели газа площадки (send/gas_mode.rs):
//! `Legacy` — одна gasPrice (BSC, как было: send/tx.rs legacy); `Eip1559` — maxFeePerGas и чаевые (тип 2,
//! send/tx_1559.rs). Сборка и подпись транзакции нужного вида, потолок цены за единицу газа (предел пары, запас
//! нативной монеты на газ, потолок отмены) и цена отмены (надбавка к обеим ценам типа 2).
//! Кто вызывает: exec/txs.rs (отправка и чаевые), send/cancel.rs и send/track.rs (отмена), send/track_util.rs (Watch).
//! От чего зависит: send/tx.rs, send/tx_1559.rs, wallet/signer.rs, num.rs, ctx.rs.
//! Настройки: нет (цены считает вызывающий: quote/gas.rs; надбавка отмены — SendCfg, прочитана до отправки).
//!
//! EIP-1559 (https://eips.ethereum.org/EIPS/eip-1559, «Specification»): баланс отправителя должен покрывать
//! gas_limit × max_fee_per_gas + value — поэтому потолок цены типа 2 для запаса на газ — maxFeePerGas. Замена
//! транзакции с тем же nonce в пуле geth требует поднять и потолок, и чаевые на PriceBump % (go-ethereum
//! core/txpool/legacypool/list.go, list.Add: thresholdFeeCap и thresholdTip) — у отмены типа 2 повышаются обе цены.

use std::fmt;

use alloy::signers::local::PrivateKeySigner;
use alloy_primitives::{Bytes, TxKind, U256};
use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::wallet::signer::{self, SignedTx};
use super::tx;
use super::tx_1559::{self, Fees};

/// Цена газа транзакции.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxPrice {
    /// gasPrice legacy-транзакции (wei) — BSC.
    Legacy(U256),
    /// maxFeePerGas и чаевые транзакции типа 2 и базовая цена блока, от которой они посчитаны (wei) — Arbitrum.
    Eip1559(Fees, U256),
}

impl fmt::Display for TxPrice {
    /// Для журнала: legacy — число wei, как было («… 1000000000 wei»); тип 2 — обе цены.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Legacy(p) => write!(f, "{p}"),
            Self::Eip1559(x, _) => write!(f, "maxFee {} / чаевые {}", x.max_fee, x.tip),
        }
    }
}

impl TxPrice {
    /// Потолок цены за единицу газа: legacy — gasPrice; тип 2 — maxFeePerGas (EIP-1559: столько кошелёк обязан
    /// иметь на газ, и не дороже этого транзакция заплатит). Ошибок нет.
    pub fn cap(&self) -> U256 {
        match self {
            Self::Legacy(p) => *p,
            Self::Eip1559(x, _) => U256::from(x.max_fee),
        }
    }

    /// Возвращает type-2 RLP для GasPriceOracle до подписи, с теми же данными, что и sign.
    pub fn unsigned(&self, ctx: &Ctx, chain: u64, nonce: u64, gas: u64, (to,value,input): (TxKind,U256,Bytes)) -> GatewayResult<Bytes> {
        use alloy::consensus::SignableTransaction;
        let Self::Eip1559(f, _) = self else {
            return Err(ctx.err(ErrorCode::BadResponse,"Base требует type-2 транзакцию"));
        };
        let tx = tx_1559::eip1559(ctx, chain, nonce, *f, gas, to, value, input)?;
        Ok(tx.encoded_for_signing().into())
    }

    /// Собирает и подписывает транзакцию нужного вида. Принимает: окружение (ошибки), подписанта, что подписываем
    /// (текст ошибки подписи: «Подпись транзакции», «Подпись отмены»), chainId, nonce, лимит газа, адрес, сумму, данные.
    /// Возвращает: байты и хеш. Ошибки: BadResponse (цена не помещается, чаевые выше потолка), Internal (подпись).
    #[allow(clippy::too_many_arguments)]
    pub fn sign(
        &self,
        ctx: &Ctx,
        key: &PrivateKeySigner,
        what: &str,
        chain: u64,
        nonce: u64,
        gas: u64,
        (to, value, input): (TxKind, U256, Bytes),
    ) -> GatewayResult<SignedTx> {
        let signed = match self {
            Self::Legacy(p) => {
                let t = tx::legacy(ctx, chain, nonce, *p, gas, to, value, input)?;
                signer::sign_with(key, t)
            }
            Self::Eip1559(f, _) => {
                let t = tx_1559::eip1559(ctx, chain, nonce, *f, gas, to, value, input)?;
                signer::sign_with(key, t)
            }
        };
        signed.map_err(|e| ctx.err(ErrorCode::Internal, format!("{what}: {e}")))
    }

    /// Цена отмены: прошлая × (1 + надбавка/100) вверх до wei — у legacy gasPrice, у типа 2 обе цены; не ниже
    /// минимума каналов (у типа 2 — по цене, которую транзакция заплатит: чаевые поднимаются до минимума сверх базы).
    /// Принимает: окружение, надбавку (%), минимум каналов (wei). Ошибки: BadResponse — число не считается.
    pub fn bumped(&self, ctx: &Ctx, pct: Decimal, floor: U256) -> GatewayResult<Self> {
        let bad = || {
            ctx.err(
                ErrorCode::BadResponse,
                format!("Цена газа отмены от {self} wei не считается"),
            )
        };
        let up = |v: U256| -> GatewayResult<U256> {
            let p = num::to_decimal(v, 0).ok_or_else(bad)?
                * (Decimal::ONE + pct / Decimal::ONE_HUNDRED);
            num::to_units(p.ceil(), 0).ok_or_else(bad)
        };
        match self {
            Self::Legacy(p) => Ok(Self::Legacy(up(*p)?.max(floor))),
            Self::Eip1559(x, base) => {
                let u128_of = |v: U256| u128::try_from(v).map_err(|_| bad());
                let f = Fees {
                    max_fee: u128_of(up(U256::from(x.max_fee))?)?,
                    tip: u128_of(up(U256::from(x.tip))?)?,
                };
                Ok(Self::Eip1559(
                    floored(f, floor, *base).ok_or_else(bad)?,
                    *base,
                ))
            }
        }
    }
}

/// Цены типа 2 не ниже минимума каналов по цене, которую транзакция заплатит (min(потолок, база + чаевые)): чаевые
/// поднимаются до (минимум − база), потолок — не ниже базы + чаевых. Принимает: цены, минимум (wei), базу блока (wei).
/// Возвращает: цены; None — число не помещается в u128. Ошибок нет.
pub fn floored(f: Fees, floor: U256, base: U256) -> Option<Fees> {
    let base = u128::try_from(base).ok()?;
    let floor = u128::try_from(floor).ok()?;
    let tip = f.tip.max(floor.saturating_sub(base));
    let max_fee = f.max_fee.max(base.checked_add(tip)?);
    Some(Fees { max_fee, tip })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Минимум канала ниже базы + чаевых — цены не меняются; выше — чаевые до минимума сверх базы, потолок не ниже.
    #[test]
    fn floor_lifts_tip_over_base() {
        let f = Fees {
            max_fee: 150,
            tip: 10,
        };
        assert_eq!(floored(f, U256::from(50u64), U256::from(100u64)), Some(f));
        let up = floored(f, U256::from(180u64), U256::from(100u64)).unwrap();
        assert_eq!(
            up,
            Fees {
                max_fee: 180,
                tip: 80
            }
        );
    }
}
