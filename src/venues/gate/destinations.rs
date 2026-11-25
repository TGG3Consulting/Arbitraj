//! Persist the exact destination Gate rejected. A saved address alone does not prove use in the web console.
use std::collections::HashMap;
use std::sync::{
    Mutex as SyncMutex,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

use super::{
    ctx::{Ctx, SECRET_API_KEY},
    errors::Op,
    num, rest,
};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use chrono::Utc;
use reqwest::Method;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OnceCell};

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    account: String,
    asset: String,
    network: String,
    address: String,
}

fn account(ctx: &Ctx) -> GatewayResult<String> {
    let key = ctx.vault.get(SECRET_API_KEY).map_err(|e| ctx.tag(e))?;
    Ok(digest(&key))
}
fn digest(key: &str) -> String {
    hex::encode(Sha256::digest(key.as_bytes()))
}
fn address_key(address: &str) -> String {
    if address.len() == 42
        && address.starts_with("0x")
        && address[2..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        address.to_ascii_lowercase()
    } else {
        address.to_owned()
    }
}
impl Key {
    fn new(ctx: &Ctx, asset: &str, network: &str, address: &str) -> GatewayResult<Self> {
        Ok(Self {
            account: account(ctx)?,
            asset: asset.into(),
            network: network.into(),
            address: address_key(address),
        })
    }
}

// Bind the result to the key actually used for signing, even when credentials change while waiting for the response.
tokio::task_local! { static EXPECTED_ACCOUNT:String; }
pub(super) fn validate_signer(ctx: &Ctx, api_key: &str) -> GatewayResult<()> {
    if EXPECTED_ACCOUNT
        .try_with(|expected| expected != &digest(api_key))
        .unwrap_or(false)
    {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::InvalidCredentials,
            None,
            "Ключ Gate изменился перед подписанием запроса",
        )));
    }
    Ok(())
}

#[derive(Default)]
pub(super) struct Destinations {
    states: OnceCell<Mutex<HashMap<Key, State>>>,
    blocked: AtomicUsize,
    sequence: AtomicU64,
    /// Kept only while a POST for the destination is outstanding.
    flights: SyncMutex<HashMap<Key, Flight>>,
    recheck: Mutex<()>,
}
#[derive(Default)]
struct Flight {
    requests: usize,
    last_applied: u64,
}
#[derive(Copy, Clone)]
struct State {
    rejected_ms: Option<i64>,
    order: u64,
    dirty: bool,
}
pub(super) struct Attempt<'a> {
    owner: &'a Destinations,
    key: Key,
    order: u64,
}
impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        let mut flights = self.owner.flights.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(f) = flights.get_mut(&self.key) {
            f.requests -= 1;
            if f.requests == 0 {
                flights.remove(&self.key);
            }
        }
    }
}
impl Attempt<'_> {
    pub(super) async fn signed<T>(&self, f: impl std::future::Future<Output = T>) -> T {
        EXPECTED_ACCOUNT.scope(self.key.account.clone(), f).await
    }
}

impl Destinations {
    async fn states(&self, ctx: &Ctx) -> GatewayResult<&Mutex<HashMap<Key, State>>> {
        self.states.get_or_try_init(||async{
            let rows=ctx.journal.db().ledger().read(|c|{
                let mut q=c.prepare("SELECT account,asset,network,address,rejected_at FROM gate_withdraw_denial")?;
                let rows=q.query_map([],|r|Ok((Key{account:r.get(0)?,asset:r.get(1)?,network:r.get(2)?,address:r.get(3)?},
                    State{order:0,rejected_ms:Some(r.get(4)?),dirty:false})))?;
                rows.collect::<rusqlite::Result<HashMap<_,_>>>()
            }).await.map_err(|e|internal(ctx,e))?;
            self.blocked.store(rows.len(),Ordering::Release);
            Ok(Mutex::new(rows))
        }).await
    }
    pub(super) async fn check(
        &self,
        ctx: &Ctx,
        asset: &str,
        network: &str,
        address: &str,
    ) -> GatewayResult<()> {
        let states = self.states(ctx).await?;
        if self.blocked.load(Ordering::Acquire) == 0 {
            return Ok(());
        }
        let key = Key::new(ctx, asset, network, address)?;
        let mut states = states.lock().await;
        if states.get(&key).is_some_and(|r| r.dirty) {
            self.flush(ctx, &mut states, &key).await?;
        }
        if states.contains_key(&key) {
            return Err(ctx.tag(GatewayError::new(ErrorCode::InvalidAddress,None,format!("Gate отклонила вывод {asset} в сети {network} на {address} (ADDRESS_NOT_USED). После успешного вывода из кабинета или верификации точного адреса нажмите «Проверить подключение» Gate."))));
        }
        Ok(())
    }

    pub(super) fn attempt<'a>(
        &'a self,
        ctx: &Ctx,
        asset: &str,
        network: &str,
        address: &str,
    ) -> GatewayResult<Attempt<'a>> {
        let key = Key::new(ctx, asset, network, address)?;
        let mut flights = self.flights.lock().unwrap_or_else(|p| p.into_inner());
        flights.entry(key.clone()).or_default().requests += 1;
        Ok(Attempt {
            owner: self,
            key,
            order: self.sequence.fetch_add(1, Ordering::Relaxed) + 1,
        })
    }

    pub(super) async fn record(
        &self,
        ctx: &Ctx,
        a: &Attempt<'_>,
        denied: bool,
    ) -> GatewayResult<()> {
        let mut states = self.states(ctx).await?.lock().await;
        {
            let mut flights = self.flights.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(f) = flights.get_mut(&a.key) {
                if f.last_applied > a.order {
                    return Ok(());
                }
                f.last_applied = a.order;
            }
        }
        if !denied && !states.contains_key(&a.key) {
            return Ok(());
        }
        states.insert(
            a.key.clone(),
            State {
                order: a.order,
                rejected_ms: denied.then(|| Utc::now().timestamp_millis()),
                dirty: true,
            },
        );
        self.blocked.store(states.len(), Ordering::Release);
        self.flush(ctx, &mut states, &a.key).await
    }

    /// Keep the in-memory refusal (or pending clear) on write failure. A later check retries the write before buying.
    async fn flush(
        &self,
        ctx: &Ctx,
        states: &mut HashMap<Key, State>,
        key: &Key,
    ) -> GatewayResult<()> {
        let when = states[key].rejected_ms;
        let k = key.clone();
        ctx.journal.db().ledger().write(move|c|{
            if let Some(at)=when {
                c.execute("INSERT INTO gate_withdraw_denial(account,asset,network,address,rejected_at) VALUES(?1,?2,?3,?4,?5)
                    ON CONFLICT(account,asset,network,address) DO UPDATE SET rejected_at=excluded.rejected_at",
                    rusqlite::params![k.account,k.asset,k.network,k.address,at])?;
            }else{
                c.execute("DELETE FROM gate_withdraw_denial WHERE account=?1 AND asset=?2 AND network=?3 AND address=?4",
                    rusqlite::params![k.account,k.asset,k.network,k.address])?;
            }
            Ok(())
        }).await.map_err(|e|internal(ctx,e))?;
        if when.is_some() {
            if let Some(state) = states.get_mut(key) {
                state.dirty = false;
            }
        } else {
            states.remove(key);
        }
        self.blocked.store(states.len(), Ordering::Release);
        Ok(())
    }

    pub(super) async fn reconcile(&self, ctx: &Ctx) -> GatewayResult<usize> {
        let states = self.states(ctx).await?;
        if self.blocked.load(Ordering::Acquire) == 0 {
            return Ok(0);
        }
        let Ok(_guard) = self.recheck.try_lock() else {
            return Ok(0);
        };
        let current = account(ctx)?;
        let pending: Vec<_> = states
            .lock()
            .await
            .iter()
            .filter(|(k, _)| k.account == current)
            .filter_map(|(k, v)| Some((k.clone(), v.rejected_ms?, v.order)))
            .collect();
        let mut cleared = 0;
        for (key, at, order) in pending {
            if EXPECTED_ACCOUNT
                .scope(current.clone(), allowed(ctx, &key, at))
                .await?
            {
                let mut rows = states.lock().await;
                if let Some(r) = rows.get_mut(&key)
                    && r.order == order
                    && r.rejected_ms == Some(at)
                {
                    r.rejected_ms = None;
                    r.dirty = true;
                    self.flush(ctx, &mut rows, &key).await?;
                    cleared += 1;
                }
            }
        }
        Ok(cleared)
    }
}

// RAIN's actual ADDRESS_NOT_USED response names two independently sufficient conditions:
// "only used addresses or verified addresses are allowed for api withdrawal".
async fn allowed(ctx: &Ctx, key: &Key, at: i64) -> GatewayResult<bool> {
    let saved = verified_address(ctx, key).await;
    if matches!(saved, Ok(true)) {
        return Ok(true);
    }
    match history_proves(ctx, key, at).await {
        Ok(true) => Ok(true),
        Ok(false) => saved,
        Err(e) => Err(e),
    }
}
fn verified(row: &Value, key: &Key) -> bool {
    row["verified"] == "1"
        && row["currency"] == key.asset
        && row["chain"] == key.network
        && row["address"]
            .as_str()
            .is_some_and(|a| address_key(a) == key.address)
        && row
            .get("tag")
            .is_none_or(|m| m.is_null() || m.as_str() == Some(""))
}

async fn verified_address(ctx: &Ctx, key: &Key) -> GatewayResult<bool> {
    let mut page = 1;
    loop {
        let p = vec![
            ("currency", key.asset.clone()),
            ("chain", key.network.clone()),
            ("verified", "1".into()),
            ("limit", "100".into()),
            ("page", page.to_string()),
        ];
        let list = rest::signed(
            ctx,
            Method::GET,
            "/wallet/saved_address",
            p,
            None,
            Op::Other,
        )
        .await?;
        let list = list
            .as_array()
            .ok_or_else(|| super::errors::bad("Список адресов Gate не массив", ""))?;
        if list.iter().any(|row| verified(row, key)) {
            return Ok(true);
        }
        if list.len() < 100 {
            return Ok(false);
        }
        page += 1;
    }
}
async fn history_proves(ctx: &Ctx, key: &Key, at: i64) -> GatewayResult<bool> {
    let start = at.div_euclid(1000);
    let mut end = Utc::now().timestamp();
    while end >= start {
        // Page by the exchange's time range and limit/offset. One UI check, only for known rejections.
        let from = start.max(end - 30 * 24 * 60 * 60 + 1);
        let mut offset = 0;
        loop {
            let params = vec![
                ("currency", key.asset.clone()),
                ("from", from.to_string()),
                ("to", end.to_string()),
                ("limit", "100".into()),
                ("offset", offset.to_string()),
            ];
            let list = rest::signed(
                ctx,
                Method::GET,
                "/wallet/withdrawals",
                params,
                None,
                Op::Other,
            )
            .await?;
            let list = list
                .as_array()
                .ok_or_else(|| super::errors::bad("История выводов Gate не массив", ""))?;
            if list.iter().any(|r| done_after(r, key, at)) {
                return Ok(true);
            }
            if list.len() < 100 {
                break;
            }
            offset += 100;
        }
        end = from - 1;
    }
    Ok(false)
}
fn done_after(r: &Value, key: &Key, at: i64) -> bool {
    r["status"] == "DONE"
        && r["currency"] == key.asset
        && r["chain"] == key.network
        && r["address"]
            .as_str()
            .is_some_and(|a| address_key(a) == key.address)
        && r.get("memo")
            .is_none_or(|m| m.is_null() || m.as_str() == Some(""))
        && num::dec_opt(r, "timestamp")
            .ok()
            .flatten()
            .is_some_and(|s| s > rust_decimal::Decimal::new(at, 3))
        && num::text_opt(r, "txid").is_some_and(|h| !h.is_empty())
}
fn internal(ctx: &Ctx, e: crate::storage::db::DbError) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::Internal,
        None,
        format!("Учёт отказа адреса вывода Gate: {e}"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn only_a_later_success_on_the_same_chain_asset_address_and_memo_clears_the_refusal() {
        let key = Key {
            account: "test".into(),
            asset: "RAIN".into(),
            network: "BSC".into(),
            address: address_key("0x00000000000000000000000000000000000000aB"),
        };
        let good = json!({"status":"DONE","currency":"RAIN","chain":"BSC","address":"0x00000000000000000000000000000000000000Ab","timestamp":"1795300000.238","txid":"tx","memo":""});
        let mut saved =
            json!({"currency":"RAIN","chain":"BSC","address":key.address,"tag":"","verified":"1"});
        assert!(verified(&saved, &key));
        saved["verified"] = json!("0");
        assert!(!verified(&saved, &key));
        saved["verified"] = json!("1");
        saved["tag"] = json!("42");
        assert!(!verified(&saved, &key));
        assert!(done_after(&good, &key, 1795300000237));
        assert!(!done_after(&good, &key, 1795300000238));
        for (field, value) in [
            ("status", "REVIEW"),
            ("currency", "NEW"),
            ("chain", "ARB"),
            ("address", "other"),
            ("memo", "tag"),
            ("timestamp", "unknown"),
            ("txid", ""),
        ] {
            let mut row = good.clone();
            row[field] = json!(value);
            assert!(!done_after(&row, &key, 1795300000237), "{field}");
        }
        assert_ne!(
            address_key("SomeBase58Address"),
            address_key("someBase58Address")
        );
    }
}
