//! Факт одиночного вызова ShibaswapV2Adapter. RETURN модуля — разница balanceOf получателя.
use super::{fill, signed_call::Erc20Call};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::{
    ctx::Ctx,
    node::rpc,
    pools::{
        abi_v2v3::IUniswapV3Pool, direct_module, exact_wallet_tokens, families::Family,
        solidly_classic_factory, solidly_v3,
    },
    send::{canonical_receipt, track::Receipt},
};
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proof {
    pub module: Address,
    pub pool: Address,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub minimum: U256,
    pub deadline: U256,
    pub module_hash: B256,
    pub exact_codes: Option<(B256, B256)>,
    #[serde(skip)]
    pub verified: Option<U256>,
}

impl Proof {
    pub(super) fn data(&self) -> Bytes {
        solidly_v3::execution_data(
            self.pool,
            self.input,
            self.output,
            self.amount,
            self.minimum,
            self.deadline,
        )
    }
    pub(in crate::venues::bsc_dex) fn signed(&self) -> Erc20Call {
        Erc20Call {
            chain: 1,
            payer: self.payer,
            module: self.module,
            calldata: keccak256(self.data()),
        }
    }
    pub(super) async fn bind(ctx: &Ctx, f: &Family, terms: &mut Self) -> GatewayResult<()> {
        let at = crate::venues::bsc_dex::pools::nami::block(ctx, "latest").await?;
        direct_module::verify_factory(ctx, f, terms.module, &at).await?;
        let code: Bytes = rpc::call(ctx, "eth_getCode", json!([terms.module, at])).await?;
        let runtime = keccak256(code);
        if !ctx
            .dex_list("dex.module_runtime_hashes", f.id)?
            .iter()
            .any(|h| h.parse::<B256>().ok() == Some(runtime))
        {
            return Err(ctx.err(
                ErrorCode::QuoteMismatch,
                "ShibaSwap: изменился код проверяемого модуля",
            ));
        }
        terms.module_hash = runtime;
        terms.exact_codes = exact_wallet_tokens::pair(ctx, f.id, terms.input, terms.output);
        Ok(())
    }

    fn movement(&self, logs: &[Log]) -> Option<(U256, U256)> {
        let mut swaps = logs.iter().filter(|l| {
            l.address() == self.pool && l.topic0() == Some(&IUniswapV3Pool::Swap::SIGNATURE_HASH)
        });
        let s = IUniswapV3Pool::Swap::decode_log_data(&swaps.next()?.inner.data).ok()?;
        let (paid, credit) = if self.input < self.output {
            (s.amount0, s.amount1)
        } else {
            (s.amount1, s.amount0)
        };
        if swaps.next().is_some()
            || (s.sender, s.recipient) != (self.module, self.payer)
            || paid.is_negative()
            || paid.into_raw() != self.amount
            || !credit.is_negative()
        {
            return None;
        }
        Some((self.amount, credit.unsigned_abs()))
    }

    fn exact(&self, logs: &[Log]) -> Option<U256> {
        let (_, gross) = self.movement(logs)?;
        let cash = |t, f, to| fill::moved_logs(logs, t, f, to);
        (cash(self.input, Some(self.payer), Some(self.pool)) == self.amount
            && cash(self.input, Some(self.payer), None) == self.amount
            && cash(self.input, None, Some(self.payer)).is_zero()
            && cash(self.output, Some(self.pool), Some(self.payer)) == gross
            && cash(self.output, None, Some(self.payer)) == gross
            && cash(self.output, Some(self.payer), None).is_zero()
            && gross >= self.minimum)
            .then_some(gross)
    }
    pub(super) async fn included(
        &mut self,
        ctx: &Ctx,
        hash: B256,
        nonce: u64,
        r: &Receipt,
    ) -> GatewayResult<()> {
        self.verified = None;
        self.signed().included(ctx, hash, nonce, r).await?;
        if r.status.to::<u64>() != 1 {
            return Ok(());
        }
        let bad = || {
            ctx.err(
                ErrorCode::OutcomeUnknown,
                "ShibaSwap: не подтверждена чистая выплата исходного модуля",
            )
        };
        let at = json!({"blockHash":r.block_hash.ok_or_else(bad)?,"requireCanonical":true});
        solidly_classic_factory::plain(ctx, self.module, self.module_hash, &at)
            .await
            .map_err(|_| bad())?;
        self.movement(&r.logs).ok_or_else(bad)?;
        if let (Some((ih, oh)), Some(net)) = (self.exact_codes, self.exact(&r.logs))
            && tokio::try_join!(
                solidly_classic_factory::plain(ctx, self.input, ih, &at),
                solidly_classic_factory::plain(ctx, self.output, oh, &at)
            )
            .is_ok()
        {
            self.verified = Some(net);
            return Ok(());
        }
        let net = super::shiba_return::read(ctx, r, hash, self.signed())
            .await
            .ok_or_else(bad)?;
        if net.is_zero() || net < self.minimum {
            return Err(bad());
        }
        canonical_receipt::verify(ctx, hash, r)
            .await
            .map_err(|_| bad())?;
        self.verified = Some(net);
        Ok(())
    }
    pub(super) fn amounts(
        &self,
        logs: &[Log],
        wallet: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        if (wallet, input, output) != (self.payer, self.input, self.output) {
            return None;
        }
        Some((self.movement(logs)?.0, self.verified?))
    }
}

#[cfg(test)]
#[path = "shiba_payment_tests.rs"]
mod tests;
