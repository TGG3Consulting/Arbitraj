//! Привязка чтений к runtime, прокси и каноническому состоянию.
use super::*;
async fn word(ctx: &Ctx, address: Address, key: B256, h: Head) -> Result<B256, String> {
    rpc::call(ctx, "eth_getStorageAt", json!([address, key, h.selector()]))
        .await
        .map_err(|e| e.message)
}
/// Привязка внешнего кода и служебных чтений подтверждается заново на каждом H.
pub(super) async fn binding(
    ctx: &Ctx,
    t: Address,
    h: Head,
    reads: Reads,
    generation: u64,
    cache: &Cache,
) -> Result<Option<(Identity, Code, Option<Code>)>, String> {
    if reads.implementation {
        let actual = word(ctx, t, IMPL_SLOT, h).await?;
        if !reads.matches_implementation(actual) {
            // Старый адрес может уже не содержать код. Сначала сравниваем указатель.
            return Ok(None);
        }
    }
    let token = cache.code(ctx, t, generation, h);
    let other = async {
        match reads.other {
            Some(a) => cache.code(ctx, a, generation, h).await.map(Some),
            None => Ok(None),
        }
    };
    let admin = async {
        if reads.admin {
            word(ctx, t, ADMIN_SLOT, h).await.map(Some)
        } else {
            Ok(None)
        }
    };
    let (token, other, admin) = tokio::try_join!(token, other, admin)?;
    let target = other.as_ref().map(|c| (c.account, keccak256(&c.bytecode)));
    Ok(Some((
        Identity {
            runtime: keccak256(&token.bytecode),
            target,
            implementation_slot: reads.implementation,
            admin,
        },
        token,
        other,
    )))
}
pub(super) async fn original(
    ctx: &Ctx,
    t: Address,
    wallet: Address,
    field: BalanceField,
    h: Head,
) -> Result<U256, String> {
    original_for(ctx, t, wallet, wallet, field, h).await
}
pub(super) async fn original_for(
    ctx: &Ctx,
    t: Address,
    wallet: Address,
    holder: Address,
    field: BalanceField,
    h: Head,
) -> Result<U256, String> {
    let tx = json!({"from":wallet,"to":t,"data":probe_calls::balance(wallet,t,holder).input});
    let read = async {
        rpc::call::<Bytes>(ctx, "eth_call", json!([tx, h.selector()]))
            .await
            .map_err(|e| e.message)
    };
    let (b, w) = tokio::try_join!(read, word(ctx, field.storage, field.key, h))?;
    let balance = balance_word(&b, w)?;
    if balance > field.max {
        return Err("старшие биты слова заняты".into());
    }
    Ok(balance)
}
