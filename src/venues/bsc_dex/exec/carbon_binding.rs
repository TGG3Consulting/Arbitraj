//! Идентичность прокси Carbon непосредственно перед включённой сделкой. Изменение реализации в том же блоке до сделки требует отдельной сверки.
use super::super::{
    ctx::Ctx,
    node::rpc,
    pools::{carbon::ICarbon, solidly_classic_factory},
    send::track::Receipt,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::{Address, B256};
use serde::{Deserialize, Serialize};
use serde_json::json;
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Code {
    pub shell: B256,
    pub implementation: Address,
    pub runtime: B256,
}
impl Code {
    pub(super) fn profile(ctx: &Ctx, id: &str) -> GatewayResult<Self> {
        let bad = || {
            ctx.err(
                ErrorCode::NotConfigured,
                "Carbon: требуется установленная реализация и хеши обеих ролей",
            )
        };
        let implementations = ctx.dex_list("dex.proxy_implementations", id)?;
        let runtimes = ctx.dex_list("dex.proxy_runtime_hashes", id)?;
        let ([implementation], [runtime]) = (implementations.as_slice(), runtimes.as_slice())
        else {
            return Err(bad());
        };
        Ok(Self {
            shell: ctx
                .dex_text("dex.factory_runtime_hash", id)?
                .parse()
                .map_err(|_| bad())?,
            implementation: implementation.parse().map_err(|_| bad())?,
            runtime: runtime.parse().map_err(|_| bad())?,
        })
    }
    pub(super) async fn at_transaction(
        &self,
        ctx: &Ctx,
        controller: Address,
        hash: B256,
        r: &Receipt,
    ) -> GatewayResult<()> {
        let fail = || {
            ctx.err(ErrorCode::OutcomeUnknown,"Carbon: не подтверждена реализация контроллера на позиции сделки в каноническом блоке")
        };
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Block {
            hash: B256,
            parent_hash: B256,
            transactions: Vec<B256>,
        }
        let block = r.block_hash.ok_or_else(fail)?;
        let index = r.transaction_index.ok_or_else(fail)?.to::<u64>();
        let (b,logs):(Option<Block>,Vec<Log>)=tokio::try_join!(
            rpc::call(ctx,"eth_getBlockByNumber",json!([r.block_number,false])),
            rpc::call(ctx,"eth_getLogs",json!([{"blockHash":block,"address":controller,"topics":[ICarbon::Upgraded::SIGNATURE_HASH]}]))
        ).map_err(|_|fail())?;
        let b = b.ok_or_else(fail)?;
        if b.hash != block
            || usize::try_from(index)
                .ok()
                .and_then(|n| b.transactions.get(n))
                != Some(&hash)
            || logs.iter().any(|l| {
                l.address() != controller
                    || l.block_hash != Some(block)
                    || l.removed
                    || l.transaction_index.is_none_or(|i| i <= index)
            })
        {
            return Err(fail());
        }
        solidly_classic_factory::bind(
            ctx,
            controller,
            self.shell,
            Some((self.implementation, self.runtime)),
            &json!({"blockHash":b.parent_hash,"requireCanonical":true}),
        )
        .await
        .map_err(|_| fail())
    }
}
