//! Сверка PropAMMRouter по исходной подписи, реализации прокси и фактическому рынку выплаты.
//! Исполнитель сверяет подписанные условия с кодом ролей в блоке фактического обмена.
#[path = "prop_wallet_payment.rs"]
mod payment;
#[path = "stelaxis_payment.rs"]
mod stelaxis_payment;
use super::{
    carbon_binding::Code,
    prop_abi::{IProp, IStrict},
    signed_call,
};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{
        ctx::Ctx, quote::v2_wallet_sim::Order,
        send::track::Receipt,
    },
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Terms {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_hash: Option<B256>,
    pub target: Address,
    pub fallback_allowed: bool,
    pub router: Address,
    pub venue: Address,
    pub vault: Address,
    pub fallback: Address,
    pub code: Code,
    pub venue_code: Option<Code>,
    pub vault_code: Option<Code>,
    #[serde(default)]
    pub via: Option<[Address; 2]>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proof {
    pub terms: Terms,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub minimum: U256,
    pub deadline: U256,
}
impl Terms {
    pub(in crate::venues::bsc_dex) fn proof(
        self,
        o: &Order,
        minimum: U256,
        deadline: U256,
    ) -> Proof {
        Proof {
            terms: self,
            payer: o.payer,
            input: o.input,
            output: o.output,
            amount: o.amount,
            minimum,
            deadline,
        }
    }
}
impl Proof {
    pub(in crate::venues::bsc_dex) fn data(&self) -> Bytes {
        let t = self.terms;
        if t.target == t.router {
            IProp::swapViaVenueV1Call {
                venue: t.venue,
                tokenIn: self.input,
                tokenOut: self.output,
                amountIn: self.amount,
                minOut: self.minimum,
                recipient: self.payer,
                deadline: self.deadline,
            }
            .abi_encode()
        } else if t.fallback_allowed {
            IStrict::executeWithFallbackCall {
                venue: t.venue,
                input: self.input,
                output: self.output,
                amount: self.amount,
                minimum: self.minimum,
                deadline: self.deadline,
                fallbackRouter: t.fallback,
            }
            .abi_encode()
        } else {
            IStrict::executeCall {
                venue: t.venue,
                input: self.input,
                output: self.output,
                amount: self.amount,
                minimum: self.minimum,
                deadline: self.deadline,
            }
            .abi_encode()
        }
        .into()
    }
    pub(super) async fn included(&self, ctx: &Ctx, hash: B256, r: &Receipt) -> GatewayResult<()> {
        signed_call::included(
            ctx,
            hash,
            r,
            self.payer,
            self.terms.target,
            U256::ZERO,
            self.data(),
        )
        .await?;
        self.verify_roles_at_fill(ctx, hash, r).await
    }
    pub(in crate::venues::bsc_dex) async fn verify_roles_at_fill(
        &self,
        ctx: &Ctx,
        hash: B256,
        r: &Receipt,
    ) -> GatewayResult<()> {
        if let Some(accepted) = self.terms.module_hash {
            let block = r.block_hash.ok_or_else(|| {
                ctx.err(
                    crate::gateway::errors::ErrorCode::OutcomeUnknown,
                    "Нет канонического блока оплаты PropVenueAdapter",
                )
            })?;
            crate::venues::bsc_dex::pools::solidly_classic_factory::plain(
                ctx,
                self.terms.target,
                accepted,
                &serde_json::json!({"blockHash":block,"requireCanonical":true}),
            )
            .await?;
        }
        self.terms
            .code
            .at_transaction(ctx, self.terms.router, hash, r, None)
            .await?;
        let source = self
            .outcome(&r.logs)
            .map(|(who, _, _)| who)
            .ok_or_else(|| {
                ctx.err(
                    crate::gateway::errors::ErrorCode::OutcomeUnknown,
                    "Источник выплаты LambdaClass не подтверждён",
                )
            })?;
        if source == self.terms.fallback {
            return Ok(());
        }
        for (code, role) in [
            (self.terms.venue_code.filter(|_| self.terms.router != self.terms.venue), self.terms.venue),
            (self.terms.vault_code, self.terms.vault),
        ] {
            if let Some(c) = code {
                c.at_transaction(ctx, role, hash, r, None).await?
            }
        }
        Ok(())
    }

}

#[cfg(test)]
#[path = "prop_kipseli_tests.rs"]
mod kipseli_tests;

#[cfg(test)]
#[path = "prop_wallet_tests.rs"]
mod tests;
