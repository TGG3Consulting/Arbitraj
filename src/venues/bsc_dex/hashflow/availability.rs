//! Последнее подтверждение balanceOf и allowance кошелька для локального скрининга. Промах планирует чтение в фоне.
use super::super::{ctx::Ctx, node::rpc, parts::Parts, wallet};
use super::sim;
use alloy_primitives::{Address, Bytes, U256, keccak256};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(in crate::venues::bsc_dex) struct Funds {
    pub who: Address,
    pub spend: U256,
    pub decimals: u8,
    pub native: U256,
}
type PerWallet = HashMap<(String, u64, Address, Address), Slot>;
#[derive(Default, Clone)]
pub(in crate::venues::bsc_dex) struct Cache(Arc<Mutex<PerWallet>>);
struct Slot {
    loading: bool,
    at: Instant,
    reading: Option<Funds>,
}
impl Cache {
    pub(in crate::venues::bsc_dex) fn get(
        &self,
        p: &Parts,
        chain: u64,
        input: Address,
        spender: Address,
        age: Duration,
    ) -> Option<Funds> {
        // Идентификатор кошелька учитывает замену секретной записи маппинга. Ключ к внешнему источнику не передаётся.
        let wallet = p.ctx.wallet().ok()?;
        let k = (wallet.secret, chain, input, spender);
        let mut slots = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = slots.get(&k)
            && (s.loading || s.at.elapsed() < age)
        {
            return s.reading.clone().filter(|_| s.at.elapsed() < age);
        }
        let start = Instant::now();
        slots.retain(|_, s| s.loading || s.at.elapsed() < age);
        slots.insert(
            k.clone(),
            Slot {
                loading: true,
                at: start,
                reading: None,
            },
        );
        let ctx = p.ctx.clone();
        let cache = self.clone();
        tokio::spawn(async move {
            let result = tokio::time::timeout(age, read(&ctx, chain, input, spender))
                .await
                .ok()
                .and_then(Result::ok);
            let mut slots = cache.0.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(s) = slots.get_mut(&k) {
                s.loading = false;
                s.reading = result;
            }
        });
        None
    }
}

async fn read(ctx: &Ctx, chain: u64, token: Address, spender: Address) -> Result<Funds, String> {
    let who = wallet::signer::address(ctx).map_err(|e| e.message)?;
    let h = sim::header(ctx, chain).await?;
    let word = async |sig: &str, args: &[Address]| -> Result<U256, String> {
        let mut data = keccak256(sig.as_bytes())[..4].to_vec();
        for a in args {
            data.extend_from_slice(&[0; 12]);
            data.extend_from_slice(a.as_slice());
        }
        let r: Bytes = rpc::call(
            ctx,
            "eth_call",
            json!([{"to":token,"data":Bytes::from(data)},sim::tag(h.hash)]),
        )
        .await
        .map_err(|_| "Не удалось проверить средства ERC-20")?;
        if r.len() != 32 {
            return Err("Некорректный ABI ERC-20".into());
        }
        Ok(U256::from_be_slice(&r))
    };
    let balance_args = [who];
    let permit_args = [who, spender];
    let native = async {
        rpc::call::<U256>(ctx, "eth_getBalance", json!([who, sim::tag(h.hash)]))
            .await
            .map_err(|_| "Не удалось подтвердить запас газа".to_owned())
    };
    let (bal, allow, decimals, native) = tokio::try_join!(
        word("balanceOf(address)", &balance_args),
        word("allowance(address,address)", &permit_args),
        sim::decimals(ctx, token, h.hash),
        native
    )?;
    Ok(Funds {
        who,
        decimals,
        spend: bal.min(allow),
        native,
    })
}
