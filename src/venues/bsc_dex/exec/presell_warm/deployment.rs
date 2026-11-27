//! Точные чтения кода и EIP-1186 на одном blockHash совместно используют все кошельки.
use super::super::presell_getter::Code;
use super::*;
use crate::venues::bsc_dex::{ctx::Ctx, node::rpc, quote::white_state::Head};

impl Cache {
    pub(super) fn binding(&self, t: Address, generation: u64, hash: B256) -> Arc<Binding> {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if s.block != Some((generation, hash)) {
            s.deployments.clear();
            s.block = Some((generation, hash));
        }
        s.deployments.entry(t).or_default().clone()
    }
    pub(in crate::venues::bsc_dex::exec) async fn code(
        &self,
        ctx: &crate::venues::bsc_dex::ctx::Ctx,
        t: Address,
        generation: u64,
        h: crate::venues::bsc_dex::quote::white_state::Head,
    ) -> Result<Code, String> {
        let cell = self.binding(t, generation, h.hash);
        let bytes = cell
            .code
            .get_or_try_init(|| async {
                crate::venues::bsc_dex::node::rpc::call::<Bytes>(
                    ctx,
                    "eth_getCode",
                    serde_json::json!([t, h.selector()]),
                )
                .await
                .map_err(|e| e.message)
            })
            .await?
            .clone();
        if bytes.is_empty() {
            return Err("в адресе чтения отсутствует код".into());
        }
        Ok(Code {
            account: t,
            bytecode: bytes,
        })
    }

    pub(in crate::venues::bsc_dex::exec) async fn account(
        &self,
        ctx: &Ctx,
        t: Address,
        generation: u64,
        h: Head,
    ) -> Result<layout::Account, String> {
        let cell = self.binding(t, generation, h.hash);
        let record = cell
            .state
            .get_or_try_init(|| async {
                let state: layout::Account = rpc::call(
                    ctx,
                    "eth_getProof",
                    serde_json::json!([t, [], h.selector()]),
                )
                .await
                .map_err(|e| e.message)?;
                if state.address != t {
                    return Err("узел вернул другой аккаунт EIP-1186".to_string());
                }
                Ok(state)
            })
            .await?;
        Ok(*record)
    }
}
