//! Получает уже выбранное направление из core::precheck. Класс токио-задачи
//! Background по умолчанию, расходы всех вызовов учитывает общий node::pace.
//! При снятии интереса Drop отменяет работу, включая ожидание очереди узла.
use super::super::{presell_layout as layout, presell_slot};
use super::{Admission, JobKey, Stamp};
use crate::core::types::Asset;
use crate::venues::bsc_dex::{
    ctx::Ctx,
    node::{call::block_tag, rpc},
    parts::Parts,
    quote::white_state,
    wallet::signer,
};
use alloy_primitives::U64;
use std::sync::Arc;
use tokio::sync::Notify;

struct Interest {
    ctx: Ctx,
    key: JobKey,
    id: u64,
}
impl Drop for Interest {
    fn drop(&mut self) {
        self.ctx
            .evm_chain
            .presell()
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .release(self.key, self.id);
    }
}
pub(in crate::venues::bsc_dex) fn candidate(p: &Parts, a: &Asset) -> Option<Box<dyn Send + Sync>> {
    if !super::applies(&p.ctx)
        || p.ctx.account.is_none()
        || presell_slot::scan(&p.ctx).ok().flatten().is_none()
    {
        return None;
    }
    if let Some(chain) = &a.chain
        && p.ctx.text("dex.chain").ok().as_deref() != Some(chain.as_str())
    {
        return None;
    }
    let key = (
        signer::address(&p.ctx).ok()?,
        a.contract?,
        p.ctx.settings.generation(),
    );
    let cache = p.ctx.evm_chain.presell();
    let mut s = cache.state.lock().unwrap_or_else(|p| p.into_inner());
    let id = match s.admit(key) {
        Admission::Existing(id) => id,
        Admission::New(id, wake) => {
            let task = tokio::spawn(worker(p.clone(), key, id, wake));
            s.jobs.get_mut(&key)?.task = Some(task.abort_handle());
            id
        }
    };
    Some(Box::new(Interest {
        ctx: p.ctx.clone(),
        key,
        id,
    }))
}
async fn worker(p: Parts, key: JobKey, id: u64, wake: Arc<Notify>) {
    let generation = key.2;
    let mut heads = p.heads.watch();
    let mut checked_chain = false;
    let cache = p.ctx.evm_chain.presell();

    loop {
        {
            if p.ctx.settings.generation() != generation || !super::applies(&p.ctx) {
                return;
            }
            let number = heads.borrow_and_update().as_ref().map(|h| h.number);
            if !checked_chain {
                match rpc::call::<U64>(&p.ctx, "eth_chainId", serde_json::json!([])).await {
                    Ok(x) if Some(x.to::<u64>()) == p.ctx.chain_id() => checked_chain = true,
                    Ok(_) => cache.failed(key, id, "фоновый узел сообщил другую сеть".into()),
                    Err(e) => {
                        cache.failed(key, id, format!("фоновая проверка сети: {}", e.message))
                    }
                }
            }
            if checked_chain && let Some(n) = number {
                let h = match white_state::head(&p.ctx, &block_tag(n)).await {
                    Ok(h) if h.number.to::<u64>() == n => Some(h),
                    Ok(_) => {
                        cache.failed(key, id, "возвращена другая высота".into());
                        None
                    }
                    Err(e) => {
                        cache.failed(key, id, format!("заголовок фона: {}", e.message));
                        None
                    }
                };
                if let Some(h) = h {
                    let stamp = Stamp {
                        job: id,
                        generation,
                        number: n,
                        hash: h.hash,
                    };
                    let (funds, negative) = prepare(&p.ctx, key, h, cache).await;
                    cache.publish(key, stamp, funds, negative);
                }
            }
        }
        tokio::select! { r=heads.changed()=>if r.is_err(){return}, ()=wake.notified()=>{} }
    }
}

async fn prepare(
    ctx: &Ctx,
    key: JobKey,
    h: white_state::Head,
    cache: &super::Cache,
) -> (super::Funds, Option<layout::Negative>) {
    let (wallet, t, generation) = key;
    if let Some(old) = cache.negative(key) {
        match old.unchanged(ctx, generation, h, cache).await {
            Ok(true) => return (Err(old.why.clone()), Some(old)),
            Ok(false) => {}
            // Неподдерживаемый/недоступный EIP-1186 не блокирует повторную
            // проверку обычными RPC, в т.ч. положительное заключение на этом H.
            Err(e) => {
                tracing::debug!(error = %e, "не удалось перепроверить снимок отказа предпродажи")
            }
        }
    }
    match layout::verify(ctx, t, wallet, h, generation, cache, cache.identity(key)).await {
        Ok(p) => (Ok((p.identity, p.field, p.balance)), None),
        Err(e) => {
            let (why, negative) = e.record(ctx, generation, h, cache).await;
            (Err(why), negative)
        }
    }
}
