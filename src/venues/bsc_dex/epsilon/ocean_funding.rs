//! Фактическая предоплата отдельного провайдера OpenOcean. Роли сверяются по каноническому блоку.
use super::super::{
    ctx::Ctx,
    exec::fill,
    node::rpc,
    pools::solidly_classic_factory::{bind, plain},
    quote::white_state::Head,
    send::track::Receipt,
};
use super::{
    ocean_role::word,
    profile::Contract,
    wire::{Plan, one},
};
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolEvent},
};
use alloy_primitives::{Address, B256, U256, keccak256};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Funding {
    pub entry: Contract,
    pub beacon: Contract,
    pub implementation: Contract,
    pub vault: Contract,
    pub vault_implementation: Contract,
}
// Третий параметр swapWithContractBalance — minAmountOut в единицах выхода этапа.
// Проверенная реализация src/LiquidFiEntry.sol: https://repo.sourcify.dev/8453/0xEc772b4019a21aF2edD567136072B90fEFf59bC0
// OpenOcean задаёт здесь и ноль. EpsilonApiWallet отдельно проверяет чистую выплату своему владельцу.
pub(super) mod abi {
    alloy::sol! {
        function swapWithContractBalance(address input,address output,uint256 minimum,address receiver,bytes terms) returns(uint256);
        event PropAMMTrade(address initiator,address executor,address receiver,address fromToken,address toToken,uint256 fromAmount,uint256 toAmount);
    }
}
impl Funding {
    pub(super) fn payment(
        &self,
        from: Address,
        assets: [Address; 2],
        executor: Address,
        target: U256,
        value: U256,
        data: &[u8],
    ) -> bool {
        let Ok(c) = abi::swapWithContractBalanceCall::abi_decode_validate(data) else {
            return false;
        };
        from == self.entry.address
            && target == U256::from_be_slice(from.as_slice())
            && value.is_zero()
            && [c.input, c.output] == assets
            && c.receiver == executor
            && c.abi_encode() == data
    }

    pub(super) fn paid(
        &self,
        p: &Plan,
        owner: Address,
        caller: Address,
        executor: Address,
        amount: U256,
        logs: &[Log],
    ) -> bool {
        let entry = self.entry.address;
        let Some(v) = one(logs, entry, abi::PropAMMTrade::SIGNATURE_HASH)
            .and_then(|l| abi::PropAMMTrade::decode_log_data(&l.inner.data).ok())
        else {
            return false;
        };
        let moved = |token, from, to| fill::moved_logs(logs, token, Some(from), Some(to));
        (
            v.initiator,
            v.executor,
            v.receiver,
            v.fromToken,
            v.toToken,
            v.fromAmount,
        ) == (owner, executor, executor, p.input, p.output, amount)
            && !v.toAmount.is_zero()
            && moved(p.input, caller, entry) == amount
            && moved(p.input, entry, self.vault.address) == amount
            && moved(p.output, self.vault.address, executor) == v.toAmount
    }

    pub(super) async fn at(&self, ctx: &Ctx, h: Head) -> bool {
        let at = h.selector();
        let beacon_slot = U256::from_be_bytes(keccak256("eip1967.proxy.beacon").0) - U256::ONE;
        let code = tokio::try_join!(
            plain(ctx, self.entry.address, self.entry.code, &at),
            plain(ctx, self.beacon.address, self.beacon.code, &at),
            plain(
                ctx,
                self.implementation.address,
                self.implementation.code,
                &at
            ),
            bind(
                ctx,
                self.vault.address,
                self.vault.code,
                Some((
                    self.vault_implementation.address,
                    self.vault_implementation.code
                )),
                &at
            )
        );
        if code.is_err() {
            return false;
        }
        let (slot, implementation, vault) = tokio::join!(
            rpc::call::<B256>(
                ctx,
                "eth_getStorageAt",
                json!([self.entry.address, beacon_slot, at])
            ),
            word(ctx, h, self.beacon.address, "implementation()", &[]),
            word(ctx, h, self.entry.address, "vault()", &[])
        );
        (slot.ok(), implementation, vault)
            == (
                Some(self.beacon.address.into_word()),
                Some(self.implementation.address.into_word()),
                Some(self.vault.address.into_word()),
            )
    }
    pub(super) async fn included(&self, ctx: &Ctx, hash: B256, r: &Receipt) -> Option<()> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Block {
            hash: B256,
            parent_hash: B256,
            transactions: Vec<B256>,
        }
        let block = r.block_hash?;
        let index = r.transaction_index?.to::<u64>();
        let (b,events):(Block,Vec<Log>)=tokio::try_join!(
            rpc::call(ctx,"eth_getBlockByNumber",json!([r.block_number,false])),
            rpc::call(ctx,"eth_getLogs",json!([{"blockHash":block,"address":[self.entry.address,self.beacon.address,self.vault.address],"topics":[[keccak256("BeaconUpgraded(address)"),keccak256("Upgraded(address)")]]}]))
        ).ok()?;
        if b.hash != block
            || b.transactions.get(usize::try_from(index).ok()?) != Some(&hash)
            || events.iter().any(|l| {
                l.block_hash != Some(block)
                    || l.removed
                    || l.transaction_index.is_none_or(|i| i <= index)
            })
        {
            return None;
        }
        let parent: Head = rpc::call(ctx, "eth_getBlockByHash", json!([b.parent_hash, false]))
            .await
            .ok()?;
        (parent.hash == b.parent_hash && self.at(ctx, parent).await).then_some(())
    }
}
