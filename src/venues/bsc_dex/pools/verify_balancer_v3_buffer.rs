//! Файл: venues/bsc_dex/pools/verify_balancer_v3_buffer.rs
//! Что делает: решение 474 — второй проход проверки пулов Balancer V3: базовые токены буферов ERC4626 хранилища для
//! токенов пула (getERC4626BufferAsset; 0 — буфера нет). С ними монета пары может быть базовым токеном обёртки пула
//! (обмен через буфер), и они же идут в развёртку пар многомонетного пула. Только если задана опорная сумма превью
//! (dex.bv3_buffer_preview_raw): без неё буферы не читаются и пары через буфер не строятся.
//! Кто вызывает: pools/verify_balancer_v3.rs.
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), abi_balancer_v3_buffer.rs, load_balancer_v3_buffer.rs
//! (preview_ref).
//! Настройки: dex.bv3_buffer_preview_raw [DEX вида balancer_v3].
//!
//! Источник: VaultExtension.sol (balancer-deployments@b55dc2b8 build-info 20241204-v3-vault) L663-L665
//! getERC4626BufferAsset (_bufferAssets[обёртка]; 0 — буфер не инициализирован); IVaultAdmin.sol L390-L395 (базовый токен
//! буфера после инициализации не меняется).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_balancer_v3_buffer::IBalancerV3Buffer as B;
use super::load_balancer_v3_buffer::preview_ref;

/// Включён ли обмен через буфер у DEX: опорная сумма превью задана и верна. Принимает: окружение, id DEX. Ошибок нет
/// (неверное значение — нет; чтение пула тогда откажет с причиной).
pub fn enabled(ctx: &Ctx, id: &str) -> bool {
    matches!(preview_ref(ctx, id), Ok(Some(_)))
}

/// Базовые токены буферов по пулам. Принимает: окружение, по пулу (хранилище, токены пула; пусто — пул без второго
/// прохода), размер части пачки. Возвращает: по пулу — ненулевые базовые токены (по порядку токенов) или сбой узла
/// (текст). Откат вызова или ответ не по ABI — у токена буфера нет. Ошибок иных нет.
pub async fn assets(
    ctx: &Ctx,
    lists: &[(Address, Vec<Address>)],
    size: usize,
) -> Vec<Result<Vec<Address>, String>> {
    let all: Vec<(Address, Bytes)> = lists
        .iter()
        .flat_map(|(vault, tokens)| {
            tokens.iter().map(|t| {
                let data = B::getERC4626BufferAssetCall { wrappedToken: *t }.abi_encode();
                (*vault, Bytes::from(data))
            })
        })
        .collect();
    let got = if all.is_empty() {
        Vec::new()
    } else {
        call::try_chunks(ctx, &all, "latest", size).await
    };
    let mut got = got.into_iter();
    let mut out = Vec::with_capacity(lists.len());
    for (_, tokens) in lists {
        let part: Vec<call::Piece> = got.by_ref().take(tokens.len()).collect();
        out.push(decide(part));
    }
    out
}

/// Разбор ответов одного пула (без сети). Принимает: ответы по токенам. Возвращает: ненулевые базовые токены или сбой
/// узла. Ошибок иных нет.
fn decide(part: Vec<call::Piece>) -> Result<Vec<Address>, String> {
    let mut out = Vec::new();
    for p in part {
        let answer = p?;
        let a = answer
            .ok()
            .and_then(|b| B::getERC4626BufferAssetCall::abi_decode_returns(&b).ok());
        if let Some(a) = a.filter(|a| !a.is_zero())
            && !out.contains(&a)
        {
            out.push(a);
        }
    }
    Ok(out)
}
